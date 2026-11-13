use crate::network::{Shown, resolve};
use crate::options::parse_defined;
use crate::output::{FAILURE, LAST_ERROR_BYTES, Output};
use crate::system::{SymbolicLinks, System};
use core::cell::RefCell;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_net::http::{self, Body, ByteRange, Coding, CookieJar, HeadError, Redirect, Url};
use eggos_png::inflate::{self as deflate, ByteSource, Window};
use eggos_runtime::errno::Errno;
use eggos_tls::chain::{NoRoots, TrustAnchors};
use eggos_tls::client::{self, Buffers, RANDOMNESS_BYTES, Session, Settings, Stopped, Transport};
use eggos_tls::pem::Certificates;

mod record;

pub const ROOTS: [&CStr; 2] = [c"/etc/ssl/roots.pem", c"/media/host/roots.pem"];
const ROOTS_PIECE: usize = 4096;
const CERTIFICATE_BYTES: usize = 8192;

const USAGE: &[u8] = b"usage: fetch [-q] [-o <file>] [-u <user:password>] [-b <cookie>] \
    [--compressed] [--proxy <url>] [--range <first>-<last>] <url> [<file>]";
const COOKIE_HEADER_BYTES: usize = 2048;
const MAX_PROXY_BYTES: usize = 512;
const PARTIAL_CONTENT: u16 = 206;
const USER_AGENT: &[u8] = b"EggOS-fetch";
const MODE: u32 = 0o644;
pub const CONNECT_TIMEOUT_MS: u32 = 10_000;
pub const RECEIVE_TIMEOUT_MS: u32 = 30_000;
pub const MAX_REDIRECTS: usize = 5;
pub const MAX_URL_BYTES: usize = 2048;
const REQUEST_BYTES: usize = MAX_URL_BYTES + 512;
const PIECE: usize = 4096;
const MAX_PATH_BYTES: usize = 4096;

struct Address {
    bytes: [u8; MAX_URL_BYTES],
    length: usize,
}

impl Address {
    const fn empty() -> Self {
        Self {
            bytes: [0; MAX_URL_BYTES],
            length: 0,
        }
    }

    fn of(parts: &[&[u8]]) -> Option<Self> {
        let mut address = Self::empty();
        for part in parts {
            let end = address.length.checked_add(part.len())?;
            address
                .bytes
                .get_mut(address.length..end)?
                .copy_from_slice(part);
            address.length = end;
        }
        Some(address)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

enum Sink<'a> {
    StandardOutput,
    File(&'a CStr),
}

struct Request<'a> {
    quiet: bool,
    no_clobber: bool,
    range: Option<ByteRange>,
    sink: Sink<'a>,
    record: Option<&'a record::Recorder<'a>>,
    credentials: Option<&'a [u8]>,
    cookie: Option<&'a [u8]>,
    compressed: bool,
    proxy: Option<Proxy<'a>>,
    no_proxy: Option<&'a [u8]>,
    jar: &'a RefCell<CookieJar>,
}

/// A proxy the request goes through (D573): its host, port and `user:password`.
#[derive(Debug, Clone, Copy)]
struct Proxy<'a> {
    host: &'a [u8],
    port: u16,
    credentials: Option<&'a [u8]>,
}

/// `http://[user:password@]host[:port][/]`: a proxy URL from `--proxy` or the environment.
fn parse_proxy(text: &[u8]) -> Option<Proxy<'_>> {
    let rest = text
        .strip_prefix(b"http://")
        .or_else(|| text.strip_prefix(b"HTTP://"))
        .unwrap_or(text);
    let rest = rest.strip_suffix(b"/").unwrap_or(rest);
    let (credentials, address) = match rest.iter().rposition(|&byte| byte == b'@') {
        Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
        None => (None, rest),
    };
    let (host, port) = match address.iter().rposition(|&byte| byte == b':') {
        Some(at) => {
            let digits = &address[at + 1..];
            let port = digits.iter().try_fold(0u32, |value, &digit| {
                digit.is_ascii_digit().then(|| value * 10 + u32::from(digit - b'0'))
            })?;
            (&address[..at], u16::try_from(port).ok().filter(|&port| port > 0)?)
        }
        None => (address, 8080),
    };
    (!host.is_empty()).then_some(Proxy {
        host,
        port,
        credentials,
    })
}

/// Whether `no_proxy` (comma-separated host names and `.domain` suffixes, `*` for all) keeps
/// `host` off the proxy.
fn bypasses_proxy(no_proxy: &[u8], host: &[u8]) -> bool {
    no_proxy
        .split(|&byte| byte == b',')
        .map(|entry| {
            let start = entry.iter().position(|byte| !byte.is_ascii_whitespace()).unwrap_or(entry.len());
            let end = entry.iter().rposition(|byte| !byte.is_ascii_whitespace()).map_or(start, |at| at + 1);
            &entry[start..end]
        })
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let suffix = entry.strip_prefix(b".").unwrap_or(entry);
            entry == b"*"
                || host.eq_ignore_ascii_case(suffix)
                || (host.len() > suffix.len()
                    && host[host.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
                    && host[host.len() - suffix.len() - 1] == b'.')
        })
}

enum Answer {
    Saved,
    Redirected,
}

pub fn fetch<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut quiet = false;
    let mut dry_run = false;
    let mut no_clobber = false;
    let mut file: Option<&CStr> = None;
    let mut range_text: Option<&CStr> = None;
    let mut credentials: Option<&CStr> = None;
    let mut cookie: Option<&CStr> = None;
    let mut compressed = false;
    let mut proxy_text: Option<&CStr> = None;
    let mut no_proxy_flag = false;
    let parsed = parse_defined(output, arguments, &commands::FETCH, |_, given| {
        match given.option.short {
            Some(b'q') => quiet = true,
            Some(b'o') => file = given.value_as_c_string(),
            Some(b'u') => credentials = given.value_as_c_string(),
            Some(b'b') => cookie = given.value_as_c_string(),
            _ => {
                dry_run |= given.is_long("dry-run");
                no_clobber |= given.is_long("no-clobber");
                compressed |= given.is_long("compressed");
                no_proxy_flag |= given.is_long("no-proxy");
                if given.is_long("range") {
                    range_text = given.value_as_c_string();
                }
                if given.is_long("proxy") {
                    proxy_text = given.value_as_c_string();
                }
            }
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let range = match range_text.map(|text| ByteRange::parse(text.to_bytes())) {
        None => None,
        Some(Some(range)) => Some(range),
        Some(None) => {
            output.coded(&errors::USAGE, &[USAGE]);
            return FAILURE;
        }
    };
    let (url, file) = match (operands.all(), file) {
        (&[url], file) => (url, file),
        (&[url, path], None) => (url, Some(path)),
        _ => {
            output.coded(&errors::USAGE, &[USAGE]);
            return FAILURE;
        }
    };
    let sink = match file {
        Some(path) if path.to_bytes() != b"-" => Sink::File(path),
        _ => Sink::StandardOutput,
    };
    if dry_run {
        return would_fetch(output, url, &sink, no_clobber);
    }
    let recorder = match sink {
        Sink::File(path) => Some(record::Recorder::start(
            &mut output.system,
            url.to_bytes(),
            path,
        )),
        Sink::StandardOutput => None,
    };
    let secure = url.to_bytes().len() >= 8 && url.to_bytes()[..8].eq_ignore_ascii_case(b"https://");
    let mut proxy_copy = [0u8; MAX_PROXY_BYTES];
    let proxy_bytes: Option<&[u8]> = match (proxy_text, no_proxy_flag) {
        (_, true) => None,
        (Some(text), false) => Some(text.to_bytes()),
        (None, false) => {
            let names: [&[u8]; 2] = match secure {
                true => [b"https_proxy", b"HTTPS_PROXY"],
                false => [b"http_proxy", b"HTTP_PROXY"],
            };
            names
                .iter()
                .find_map(|name| output.system.variable(name))
                .map(|value| {
                    let length = value.len().min(MAX_PROXY_BYTES);
                    proxy_copy[..length].copy_from_slice(&value[..length]);
                    &proxy_copy[..length]
                })
        }
    };
    let proxy = match proxy_bytes.filter(|bytes| !bytes.is_empty()) {
        None => None,
        Some(bytes) => match parse_proxy(bytes) {
            Some(proxy) => Some(proxy),
            None => {
                output.coded(&errors::USAGE, &[b"invalid proxy URL"]);
                return FAILURE;
            }
        },
    };
    let mut no_proxy_copy = [0u8; MAX_PROXY_BYTES];
    let no_proxy = [&b"no_proxy"[..], b"NO_PROXY"]
        .iter()
        .find_map(|name| output.system.variable(name))
        .map(|value| {
            let length = value.len().min(MAX_PROXY_BYTES);
            no_proxy_copy[..length].copy_from_slice(&value[..length]);
            length
        })
        .map(|length| &no_proxy_copy[..length]);
    let jar = RefCell::new(CookieJar::default());
    let request = Request {
        quiet,
        no_clobber,
        range,
        sink,
        record: recorder.as_ref(),
        credentials: credentials.map(CStr::to_bytes),
        cookie: cookie.map(CStr::to_bytes),
        compressed,
        proxy,
        no_proxy,
        jar: &jar,
    };
    let status = fetch_url(output, url, &request);
    recorded(output, recorder.as_ref(), status)
}

fn would_fetch<S: System>(
    output: &mut Output<S>,
    url: &CStr,
    sink: &Sink<'_>,
    no_clobber: bool,
) -> i32 {
    let Some(address) = Address::of(&[url.to_bytes()]) else {
        output.error(&[b"the URL is longer than 2048 bytes"]);
        return FAILURE;
    };
    if let Err(error) = http::parse_url(address.as_bytes()) {
        output.error(&[address.as_bytes(), b": ", error.description().as_bytes()]);
        return FAILURE;
    }
    let path = match *sink {
        Sink::StandardOutput => {
            output.write(b"would download ");
            output.write(url.to_bytes());
            output.write(b" to standard output\n");
            return 0;
        }
        Sink::File(path) => path,
    };
    let replaced = match destination(&mut output.system, path) {
        Ok(Some(_)) if no_clobber => {
            output.file_error(path, Errno::EEXIST);
            return FAILURE;
        }
        Ok(replaced) => replaced,
        Err(errno) => {
            output.file_error(path, errno);
            return FAILURE;
        }
    };
    output.write(b"would download ");
    output.write(url.to_bytes());
    output.write(b" to ");
    output.write(path.to_bytes());
    if let Some(size) = replaced {
        output.write(b", replacing the file there (");
        output.write_number(size, 0);
        output.write(b" bytes)");
    }
    output.write(b"\n");
    0
}

fn destination<S: System>(system: &mut S, path: &CStr) -> Result<Option<u64>, Errno> {
    match system.status(None, path, SymbolicLinks::Follow) {
        Ok(status) if status.is_directory() => Err(Errno::EISDIR),
        Ok(status) => system.may_write(path).map(|()| Some(status.size)),
        Err(Errno::ENOENT) => {
            let mut held = [0u8; MAX_PATH_BYTES + 1];
            let folder = parent_folder(path.to_bytes(), &mut held).ok_or(Errno::ENAMETOOLONG)?;
            match system.status(None, folder, SymbolicLinks::Follow) {
                Ok(status) if status.is_directory() => system.may_write(folder).map(|()| None),
                Ok(_) => Err(Errno::ENOTDIR),
                Err(errno) => Err(errno),
            }
        }
        Err(errno) => Err(errno),
    }
}

fn parent_folder<'h>(path: &[u8], held: &'h mut [u8; MAX_PATH_BYTES + 1]) -> Option<&'h CStr> {
    let folder: &[u8] = match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => b"/",
        Some(at) => &path[..at],
        None => b".",
    };
    let room = held.get_mut(..folder.len() + 1)?;
    room[..folder.len()].copy_from_slice(folder);
    room[folder.len()] = 0;
    CStr::from_bytes_with_nul(room).ok()
}

fn fetch_url<S: System>(output: &mut Output<S>, url: &CStr, request: &Request<'_>) -> i32 {
    let Some(mut address) = Address::of(&[url.to_bytes()]) else {
        output.error(&[b"the URL is longer than 2048 bytes"]);
        return FAILURE;
    };
    let mut redirect = Address::empty();
    for _ in 0..=MAX_REDIRECTS {
        let url = match http::parse_url(address.as_bytes()) {
            Ok(url) => url,
            Err(error) => {
                output.error(&[address.as_bytes(), b": ", error.description().as_bytes()]);
                return FAILURE;
            }
        };
        match get(output, &url, request, &mut redirect) {
            Ok(Answer::Saved) => return 0,
            Ok(Answer::Redirected) => core::mem::swap(&mut address, &mut redirect),
            Err(status) => return status,
        }
    }
    output.error(&[url.to_bytes(), b": more than 5 redirects"]);
    FAILURE
}

fn recorded<S: System>(
    output: &mut Output<S>,
    recorder: Option<&record::Recorder<'_>>,
    status: i32,
) -> i32 {
    let Some(recorder) = recorder else {
        return status;
    };
    if status == 0 {
        recorder.done(&mut output.system);
    } else if output.system.termination_requested() {
        let words: &[u8] = match recorder.cancelled(&mut output.system) {
            record::PartialFile::Removed => b"cancelled: the partial file was removed",
            record::PartialFile::NeverCreated => b"cancelled before anything was saved",
        };
        output.error(&[words]);
    } else {
        let mut message = [0u8; LAST_ERROR_BYTES];
        let length = output.last_error().len().min(LAST_ERROR_BYTES);
        message[..length].copy_from_slice(&output.last_error()[..length]);
        recorder.failed(&mut output.system, &message[..length]);
    }
    status
}

fn authority(url: &Url<'_>) -> Shown<264> {
    Shown::of(format_args!(
        "{}:{}",
        core::str::from_utf8(url.host).unwrap_or("?"),
        url.port
    ))
}

struct Socket<'r, 'o, S: System> {
    output: &'r RefCell<&'o mut Output<S>>,
    descriptor: usize,
    shown: &'r Shown<264>,
}

impl<S: System> Transport for Socket<'_, '_, S> {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        let mut output = self.output.borrow_mut();
        match output.system.send(self.descriptor, bytes) {
            Ok(()) => Ok(()),
            Err(errno) => {
                output.error(&[self.shown.as_bytes(), b": ", errno.message()]);
                Err(Stopped)
            }
        }
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        let mut output = self.output.borrow_mut();
        receive(&mut **output, self.descriptor, buffer, self.shown).map_err(|_| Stopped)
    }
}

pub(crate) struct RootsFile<'r, 'o, S: System> {
    pub(crate) output: &'r RefCell<&'o mut Output<S>>,
}

impl<S: System> TrustAnchors for RootsFile<'_, '_, S> {
    fn each(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots> {
        let mut output = self.output.borrow_mut();
        let descriptor = ROOTS
            .iter()
            .find_map(|path| output.system.open(path).ok())
            .ok_or(NoRoots)?;
        let mut der = [0u8; CERTIFICATE_BYTES];
        let mut decoder = Certificates::new(&mut der);
        let mut text = [0u8; ROOTS_PIECE];
        let found = loop {
            match output.system.read(descriptor, &mut text) {
                Ok(0) => break decoder.finish(visit),
                Ok(count) if decoder.feed(&text[..count], visit) => break true,
                Ok(_) | Err(Errno::EINTR) => {}
                Err(_) => break false,
            }
        };
        output.system.close(descriptor);
        Ok(found)
    }
}

struct Link<'b> {
    descriptor: usize,
    session: Option<Session<'b>>,
}

pub(crate) fn tls_failure<S: System>(
    output: &mut Output<S>,
    shown: &Shown<264>,
    error: client::Error,
) -> i32 {
    match error {
        client::Error::Transport => {}
        client::Error::Alert(code) => {
            let code = Shown::<4>::of(code);
            output.error(&[
                shown.as_bytes(),
                b": ",
                error.description().as_bytes(),
                b" (",
                code.as_bytes(),
                b")",
            ]);
        }
        _ => output.error(&[shown.as_bytes(), b": ", error.description().as_bytes()]),
    }
    FAILURE
}

fn handshake<'b, S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    url: &Url<'_>,
    shown: &Shown<264>,
    buffers: &'b mut Buffers,
) -> Result<Session<'b>, i32> {
    let mut randomness = [0u8; RANDOMNESS_BYTES];
    if let Err(errno) = output.system.random(&mut randomness) {
        output.error(&[b"getrandom: ", errno.message()]);
        return Err(FAILURE);
    }
    let settings = Settings {
        host: url.host,
        now: output.system.now() as i64,
    };
    let cell = RefCell::new(output);
    let mut socket = Socket {
        output: &cell,
        descriptor,
        shown,
    };
    let mut roots = RootsFile { output: &cell };
    let connected = client::connect(&mut socket, buffers, &settings, &randomness, &mut roots);
    let output = cell.into_inner();
    connected.map_err(|error| tls_failure(output, shown, error))
}

fn send_on<S: System>(
    output: &mut Output<S>,
    link: &mut Link<'_>,
    bytes: &[u8],
    shown: &Shown<264>,
) -> Result<(), i32> {
    let Some(session) = link.session.as_mut() else {
        return output.system.send(link.descriptor, bytes).map_err(|errno| {
            output.error(&[shown.as_bytes(), b": ", errno.message()]);
            FAILURE
        });
    };
    let cell = RefCell::new(output);
    let mut socket = Socket {
        output: &cell,
        descriptor: link.descriptor,
        shown,
    };
    let sent = session.write(&mut socket, bytes);
    sent.map_err(|error| tls_failure(cell.into_inner(), shown, error))
}

fn receive_on<S: System>(
    output: &mut Output<S>,
    link: &mut Link<'_>,
    buffer: &mut [u8],
    shown: &Shown<264>,
) -> Result<usize, i32> {
    let Some(session) = link.session.as_mut() else {
        return receive(output, link.descriptor, buffer, shown);
    };
    let cell = RefCell::new(output);
    let mut socket = Socket {
        output: &cell,
        descriptor: link.descriptor,
        shown,
    };
    let received = session.read(&mut socket, buffer);
    received.map_err(|error| tls_failure(cell.into_inner(), shown, error))
}

fn get<S: System>(
    output: &mut Output<S>,
    url: &Url<'_>,
    request: &Request<'_>,
    redirect: &mut Address,
) -> Result<Answer, i32> {
    let proxy = request
        .proxy
        .filter(|_| !request.no_proxy.is_some_and(|list| bypasses_proxy(list, url.host)));
    let address = match proxy {
        Some(proxy) => resolve(output, proxy.host)?,
        None => resolve(output, url.host)?,
    };
    let descriptor = match output.system.open_stream_socket() {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.error(&[b"socket: ", errno.message()]);
            return Err(FAILURE);
        }
    };
    let mut buffers = match url.secure {
        true => Some(Buffers::new()),
        false => None,
    };
    let answer = exchange(
        output,
        descriptor,
        address,
        url,
        request,
        proxy,
        buffers.as_mut(),
        redirect,
    );
    output.system.close(descriptor);
    answer
}

fn exchange<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    address: eggos_net::Ipv4Address,
    url: &Url<'_>,
    request: &Request<'_>,
    proxy: Option<Proxy<'_>>,
    buffers: Option<&mut Buffers>,
    redirect: &mut Address,
) -> Result<Answer, i32> {
    let shown = authority(url);
    let port = proxy.map_or(url.port, |proxy| proxy.port);
    let connected = output
        .system
        .connect(descriptor, address, port, CONNECT_TIMEOUT_MS);
    match connected {
        Ok(()) => {}
        Err(Errno::EINTR) if output.system.termination_requested() => return Err(FAILURE),
        Err(errno) => {
            output.error(&[shown.as_bytes(), b": ", errno.message()]);
            return Err(FAILURE);
        }
    }
    let mut link = Link {
        descriptor,
        session: None,
    };
    if let (Some(proxy), true) = (proxy, url.secure) {
        tunnel(output, &mut link, url, proxy, &shown)?;
    }
    if let Some(buffers) = buffers {
        link.session = Some(handshake(output, descriptor, url, &shown, buffers)?);
    }
    let mut cookies = [0u8; COOKIE_HEADER_BYTES];
    let mut cookie_length = request.jar.borrow().header_for(url, &mut cookies);
    if let Some(given) = request.cookie {
        let separator: &[u8] = if cookie_length > 0 { b"; " } else { b"" };
        if cookie_length + separator.len() + given.len() <= COOKIE_HEADER_BYTES {
            cookies[cookie_length..cookie_length + separator.len()].copy_from_slice(separator);
            cookie_length += separator.len();
            cookies[cookie_length..cookie_length + given.len()].copy_from_slice(given);
            cookie_length += given.len();
        }
    }
    let options = http::RequestOptions {
        range: request.range,
        through_proxy: proxy.is_some() && !url.secure,
        credentials: request.credentials,
        proxy_credentials: proxy.and_then(|proxy| proxy.credentials).filter(|_| !url.secure),
        cookies: (cookie_length > 0).then_some(&cookies[..cookie_length]),
        compressed: request.compressed,
    };
    let mut text = [0u8; REQUEST_BYTES];
    let Some(length) = http::write_request_with(&mut text, url, USER_AGENT, &options) else {
        output.error(&[b"the URL is too long for a request"]);
        return Err(FAILURE);
    };
    send_on(output, &mut link, &text[..length], &shown)?;
    let mut head = [0u8; http::MAX_HEAD_BYTES];
    let mut filled = 0;
    let head_length = loop {
        if let Some(end) = http::head_end(&head[..filled]) {
            let interim = http::parse_head(&head[..end]).is_ok_and(|parsed| parsed.is_interim());
            if !interim {
                break end;
            }
            head.copy_within(end..filled, 0);
            filled -= end;
            continue;
        }
        if filled == head.len() {
            output.error(&[
                shown.as_bytes(),
                b": the server's answer has a head over 16 KiB",
            ]);
            return Err(FAILURE);
        }
        match receive_on(output, &mut link, &mut head[filled..], &shown)? {
            0 => {
                output.error(&[
                    shown.as_bytes(),
                    b": the server closed the connection without answering",
                ]);
                return Err(FAILURE);
            }
            count => filled += count,
        }
    };
    let parsed = match http::parse_head(&head[..head_length]) {
        Ok(parsed) => parsed,
        Err(error) => return Err(head_error(output, &shown, error)),
    };
    for value in http::headers(&head[..head_length], b"set-cookie") {
        request.jar.borrow_mut().take(url, value);
    }
    let coding = http::content_coding(&head[..head_length]);
    if coding == Coding::Other {
        output.error(&[shown.as_bytes(), b": the server compressed the file in a way fetch does not read"]);
        return Err(FAILURE);
    }
    if parsed.is_redirect()
        && let Some(location) = parsed.location
    {
        let next = match http::redirect(location) {
            Ok(Redirect::Absolute(_)) => Address::of(&[location]),
            Ok(Redirect::Path(path)) => {
                let port = Shown::<8>::of(url.port);
                let scheme: &[u8] = match url.secure {
                    true => b"https://",
                    false => b"http://",
                };
                Address::of(&[scheme, url.host, b":", port.as_bytes(), path])
            }
            Err(error) => {
                let location = Shown::<MAX_URL_BYTES>::printable(location);
                output.error(&[location.as_bytes(), b": ", error.description().as_bytes()]);
                return Err(FAILURE);
            }
        };
        return match next {
            Some(next) => {
                *redirect = next;
                Ok(Answer::Redirected)
            }
            None => {
                output.error(&[b"the redirect's URL is longer than 2048 bytes"]);
                Err(FAILURE)
            }
        };
    }
    let status = Shown::<4>::of(parsed.status);
    if parsed.status >= 400 {
        output.error(&[
            shown.as_bytes(),
            url.target,
            b": ",
            status.as_bytes(),
            b" ",
            Shown::<64>::printable(parsed.reason).as_bytes(),
        ]);
        return Err(FAILURE);
    }
    if request.range.is_some() && parsed.status != PARTIAL_CONTENT {
        output.error(&[
            shown.as_bytes(),
            url.target,
            b": the server sent the whole file, not the range asked for",
        ]);
        return Err(FAILURE);
    }
    let mut body = Body::new(parsed.framing);
    if let Some(record) = request.record {
        record.set_total(match parsed.framing {
            http::Framing::Length(length) => Some(length),
            _ => None,
        });
    }
    let sink = match request.sink {
        Sink::StandardOutput => None,
        Sink::File(path) => {
            let created = match request.no_clobber {
                true => output.system.create_new(path, MODE),
                false => output.system.create(path, MODE, true),
            };
            match created {
                Ok(file) => {
                    if let Some(record) = request.record {
                        record.output_created();
                    }
                    Some((file, path))
                }
                Err(errno) => {
                    output.file_error(path, errno);
                    return Err(FAILURE);
                }
            }
        }
    };
    let reason = Shown::<64>::printable(parsed.reason);
    let leftover_start = head_length;
    let saved = match coding {
        Coding::Identity => save(
            output,
            &mut link,
            &mut body,
            &head[leftover_start..filled],
            sink,
            &shown,
            request.record,
        ),
        coding => save_decoded(
            output,
            &mut link,
            &mut body,
            &head[leftover_start..filled],
            sink,
            &shown,
            coding,
        ),
    };
    if let Some((file, _)) = sink {
        output.system.close(file);
    }
    let saved = saved?;
    if let (Some((_, path)), false) = (sink, request.quiet) {
        output.write(b"Saved ");
        output.write_number(saved, 0);
        output.write(b" bytes to ");
        output.write(path.to_bytes());
        output.write(b" (HTTP ");
        output.write(status.as_bytes());
        if !reason.as_bytes().is_empty() {
            output.write(b" ");
            output.write(reason.as_bytes());
        }
        output.write(b")\n");
    }
    Ok(Answer::Saved)
}

/// Asks `proxy` for a tunnel to `url`'s host (`CONNECT`, D573), reading its answer's head;
/// the TLS handshake then goes through the tunnel.
fn tunnel<S: System>(
    output: &mut Output<S>,
    link: &mut Link<'_>,
    url: &Url<'_>,
    proxy: Proxy<'_>,
    shown: &Shown<264>,
) -> Result<(), i32> {
    let mut text = [0u8; REQUEST_BYTES];
    let Some(length) = http::write_connect(&mut text, url.host, url.port, proxy.credentials) else {
        output.error(&[b"the host is too long for a proxy request"]);
        return Err(FAILURE);
    };
    send_on(output, link, &text[..length], shown)?;
    let mut head = [0u8; http::MAX_HEAD_BYTES];
    let mut filled = 0;
    let end = loop {
        if let Some(end) = http::head_end(&head[..filled]) {
            break end;
        }
        if filled == head.len() {
            output.error(&[shown.as_bytes(), b": the proxy's answer has a head over 16 KiB"]);
            return Err(FAILURE);
        }
        match receive_on(output, link, &mut head[filled..], shown)? {
            0 => {
                output.error(&[shown.as_bytes(), b": the proxy closed the connection"]);
                return Err(FAILURE);
            }
            count => filled += count,
        }
    };
    match http::parse_head(&head[..end]) {
        Ok(parsed) if (200..300).contains(&parsed.status) => Ok(()),
        Ok(parsed) => {
            let status = Shown::<4>::of(parsed.status);
            output.error(&[shown.as_bytes(), b": the proxy refused the tunnel: ", status.as_bytes()]);
            Err(FAILURE)
        }
        Err(error) => Err(head_error(output, shown, error)),
    }
}

/// Where a compressed body's bytes come from: the bytes after the head, then the connection,
/// through the body's framing (D573).
struct BodySource<'r, 'o, 'l, S: System> {
    output: &'r RefCell<&'o mut Output<S>>,
    link: &'r mut Link<'l>,
    body: &'r mut Body,
    shown: &'r Shown<264>,
    first: &'r [u8],
    pending: [u8; PIECE],
    length: usize,
    at: usize,
    failed: bool,
}

impl<S: System> BodySource<'_, '_, '_, S> {
    fn refill(&mut self) -> bool {
        let mut piece = [0u8; PIECE];
        loop {
            if self.body.is_done() {
                return false;
            }
            let input: &[u8] = if !self.first.is_empty() {
                let first = self.first;
                self.first = &[];
                first
            } else {
                let mut output = self.output.borrow_mut();
                match receive_on(&mut **output, self.link, &mut piece, self.shown) {
                    Ok(0) => {
                        if let Err(error) = self.body.finish() {
                            output.error(&[self.shown.as_bytes(), b": ", error.description().as_bytes()]);
                            self.failed = true;
                        }
                        return false;
                    }
                    Ok(count) => &piece[..count],
                    Err(_) => {
                        self.failed = true;
                        return false;
                    }
                }
            };
            let mut input = input;
            self.length = 0;
            self.at = 0;
            while !input.is_empty() && !self.body.is_done() {
                let decoded = match self.body.decode(input) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        let mut output = self.output.borrow_mut();
                        output.error(&[self.shown.as_bytes(), b": ", error.description().as_bytes()]);
                        self.failed = true;
                        return false;
                    }
                };
                let room = PIECE - self.length;
                let count = decoded.data.len().min(room);
                self.pending[self.length..self.length + count].copy_from_slice(&decoded.data[..count]);
                self.length += count;
                if decoded.used == 0 || count < decoded.data.len() {
                    break;
                }
                input = &input[decoded.used..];
            }
            if self.length > 0 {
                return true;
            }
        }
    }
}

impl<S: System> ByteSource for BodySource<'_, '_, '_, S> {
    fn next_byte(&mut self) -> Option<u8> {
        if self.at == self.length && !self.refill() {
            return None;
        }
        let byte = self.pending[self.at];
        self.at += 1;
        Some(byte)
    }
}

/// Where a decompressed body goes, with its running checksums.
struct DecodedSink<'r, 'o, 'p, S: System> {
    output: &'r RefCell<&'o mut Output<S>>,
    sink: Option<(usize, &'p CStr)>,
    written: u64,
    crc: u32,
    adler: eggos_png::Adler32,
    failed: bool,
}

impl<S: System> deflate::Sink for DecodedSink<'_, '_, '_, S> {
    fn write(&mut self, bytes: &[u8]) -> bool {
        let mut output = self.output.borrow_mut();
        if write_out(&mut **output, self.sink, bytes).is_err() {
            self.failed = true;
            return false;
        }
        self.written += bytes.len() as u64;
        self.crc = eggos_png::crc32_update(self.crc, bytes);
        self.adler.update(bytes);
        true
    }
}

/// Saves a gzip or zlib (`deflate`) body decompressed (D573): its container's header read and
/// checked, the DEFLATE stream inflated through a 32 KiB window, and its trailer's checksum
/// and length compared.
fn save_decoded<S: System>(
    output: &mut Output<S>,
    link: &mut Link<'_>,
    body: &mut Body,
    first: &[u8],
    sink: Option<(usize, &CStr)>,
    shown: &Shown<264>,
    coding: Coding,
) -> Result<u64, i32> {
    let cell = RefCell::new(output);
    let mut source = BodySource {
        output: &cell,
        link,
        body,
        shown,
        first,
        pending: [0; PIECE],
        length: 0,
        at: 0,
        failed: false,
    };
    let mut target = DecodedSink {
        output: &cell,
        sink,
        written: 0,
        crc: 0,
        adler: eggos_png::Adler32::new(),
        failed: false,
    };
    let header_ok = match coding {
        Coding::Gzip => gzip_header(&mut source),
        _ => zlib_header(&mut source),
    };
    let mut window = Window::new();
    let inflated = match header_ok {
        true => deflate::inflate(&mut source, &mut window, &mut target).ok(),
        false => None,
    };
    let mut trailer = [0u8; 8];
    let trailer_length = match coding {
        Coding::Gzip => 8,
        _ => 4,
    };
    let mut complete = inflated.is_some();
    for slot in &mut trailer[..trailer_length] {
        match source.next_byte() {
            Some(byte) => *slot = byte,
            None => complete = false,
        }
    }
    let checks = complete
        && match coding {
            Coding::Gzip => {
                u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]) == target.crc
                    && u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]])
                        == target.written as u32
            }
            _ => u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]) == target.adler.value(),
        };
    let failed = source.failed || target.failed;
    let written = target.written;
    let output = cell.into_inner();
    if failed {
        return Err(FAILURE);
    }
    if !checks {
        output.error(&[shown.as_bytes(), b": the compressed file is damaged"]);
        return Err(FAILURE);
    }
    Ok(written)
}

fn gzip_header(source: &mut impl ByteSource) -> bool {
    const TEXT_FLAGS_EXTRA: u8 = 4;
    const NAME: u8 = 8;
    const COMMENT: u8 = 16;
    const HEADER_CRC: u8 = 2;
    let mut fixed = [0u8; 10];
    for slot in &mut fixed {
        let Some(byte) = source.next_byte() else {
            return false;
        };
        *slot = byte;
    }
    if fixed[..3] != [0x1f, 0x8b, 8] {
        return false;
    }
    let flags = fixed[3];
    if flags & TEXT_FLAGS_EXTRA != 0 {
        let (Some(low), Some(high)) = (source.next_byte(), source.next_byte()) else {
            return false;
        };
        for _ in 0..u16::from_le_bytes([low, high]) {
            if source.next_byte().is_none() {
                return false;
            }
        }
    }
    for field in [NAME, COMMENT] {
        if flags & field != 0 {
            loop {
                match source.next_byte() {
                    Some(0) => break,
                    Some(_) => {}
                    None => return false,
                }
            }
        }
    }
    if flags & HEADER_CRC != 0 && (source.next_byte().is_none() || source.next_byte().is_none()) {
        return false;
    }
    true
}

fn zlib_header(source: &mut impl ByteSource) -> bool {
    let (Some(method), Some(flags)) = (source.next_byte(), source.next_byte()) else {
        return false;
    };
    method & 0x0f == 8 && (u16::from(method) << 8 | u16::from(flags)) % 31 == 0 && flags & 0x20 == 0
}

fn head_error<S: System>(output: &mut Output<S>, shown: &Shown<264>, error: HeadError) -> i32 {
    output.error(&[shown.as_bytes(), b": ", error.description().as_bytes()]);
    FAILURE
}

fn receive<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    buffer: &mut [u8],
    shown: &Shown<264>,
) -> Result<usize, i32> {
    loop {
        match output
            .system
            .receive(descriptor, buffer, RECEIVE_TIMEOUT_MS)
        {
            Ok(Some(count)) => return Ok(count),
            Ok(None) => {
                output.error(&[
                    shown.as_bytes(),
                    b": the server stopped answering for 30 seconds",
                ]);
                return Err(FAILURE);
            }
            Err(Errno::EINTR) if output.system.termination_requested() => return Err(FAILURE),
            Err(Errno::EINTR) => {}
            Err(errno) => {
                output.error(&[shown.as_bytes(), b": ", errno.message()]);
                return Err(FAILURE);
            }
        }
    }
}

fn write_out<S: System>(
    output: &mut Output<S>,
    sink: Option<(usize, &CStr)>,
    data: &[u8],
) -> Result<(), i32> {
    match sink {
        None => {
            output.write(data);
            Ok(())
        }
        Some((file, path)) => output.system.write(file, data).map_err(|errno| {
            output.file_error(path, errno);
            FAILURE
        }),
    }
}

fn feed<S: System>(
    output: &mut Output<S>,
    body: &mut Body,
    mut input: &[u8],
    sink: Option<(usize, &CStr)>,
    shown: &Shown<264>,
) -> Result<u64, i32> {
    let mut saved = 0;
    while !input.is_empty() && !body.is_done() {
        let decoded = match body.decode(input) {
            Ok(decoded) => decoded,
            Err(error) => {
                output.error(&[shown.as_bytes(), b": ", error.description().as_bytes()]);
                return Err(FAILURE);
            }
        };
        write_out(output, sink, decoded.data)?;
        saved += decoded.data.len() as u64;
        if decoded.used == 0 {
            break;
        }
        input = &input[decoded.used..];
    }
    Ok(saved)
}

fn save<S: System>(
    output: &mut Output<S>,
    link: &mut Link<'_>,
    body: &mut Body,
    first: &[u8],
    sink: Option<(usize, &CStr)>,
    shown: &Shown<264>,
    record: Option<&record::Recorder<'_>>,
) -> Result<u64, i32> {
    let mut saved = feed(output, body, first, sink, shown)?;
    if let Some(record) = record {
        record.progress(&mut output.system, saved);
    }
    if output.system.termination_requested() {
        return Err(FAILURE);
    }
    let mut piece = [0u8; PIECE];
    while !body.is_done() {
        let count = receive_on(output, link, &mut piece, shown)?;
        if count == 0 {
            if let Err(error) = body.finish() {
                output.error(&[shown.as_bytes(), b": ", error.description().as_bytes()]);
                return Err(FAILURE);
            }
            break;
        }
        saved += feed(output, body, &piece[..count], sink, shown)?;
        if let Some(record) = record {
            record.progress(&mut output.system, saved);
        }
        if output.system.termination_requested() {
            return Err(FAILURE);
        }
    }
    Ok(saved)
}

#[cfg(test)]
mod tests;
