use crate::network::{Shown, resolve};
use crate::options::parse_defined;
use crate::output::{FAILURE, LAST_ERROR_BYTES, Output};
use crate::system::{SymbolicLinks, System};
use core::cell::RefCell;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_net::http::{self, Body, ByteRange, HeadError, Redirect, Url};
use eggos_runtime::errno::Errno;
use eggos_tls::chain::{NoRoots, TrustAnchors};
use eggos_tls::client::{self, Buffers, RANDOMNESS_BYTES, Session, Settings, Stopped, Transport};
use eggos_tls::pem::Certificates;

mod record;

pub const ROOTS: [&CStr; 2] = [c"/etc/ssl/roots.pem", c"/media/host/roots.pem"];
const ROOTS_PIECE: usize = 4096;
const CERTIFICATE_BYTES: usize = 8192;

const USAGE: &[u8] = b"usage: fetch [-q] [-o <file>] [--range <first>-<last>] <url> [<file>]";
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
    let parsed = parse_defined(output, arguments, &commands::FETCH, |_, given| {
        match given.option.short {
            Some(b'q') => quiet = true,
            Some(b'o') => file = given.value_as_c_string(),
            _ => {
                dry_run |= given.is_long("dry-run");
                no_clobber |= given.is_long("no-clobber");
                if given.is_long("range") {
                    range_text = given.value_as_c_string();
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
    let request = Request {
        quiet,
        no_clobber,
        range,
        sink,
        record: recorder.as_ref(),
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
    let address = resolve(output, url.host)?;
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
    buffers: Option<&mut Buffers>,
    redirect: &mut Address,
) -> Result<Answer, i32> {
    let shown = authority(url);
    let connected = output
        .system
        .connect(descriptor, address, url.port, CONNECT_TIMEOUT_MS);
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
    if let Some(buffers) = buffers {
        link.session = Some(handshake(output, descriptor, url, &shown, buffers)?);
    }
    let mut text = [0u8; REQUEST_BYTES];
    let Some(length) = http::write_ranged_request(&mut text, url, USER_AGENT, request.range) else {
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
    let saved = save(
        output,
        &mut link,
        &mut body,
        &head[leftover_start..filled],
        sink,
        &shown,
        request.record,
    );
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
