use crate::network::{Shown, resolve};
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::cell::RefCell;
use core::ffi::CStr;
use eggos_net::http::{self, Body, HeadError, Redirect, Url};
use eggos_runtime::errno::Errno;
use eggos_tls::chain::{NoRoots, TrustAnchors};
use eggos_tls::client::{self, Buffers, RANDOMNESS_BYTES, Session, Settings, Stopped, Transport};
use eggos_tls::pem::Certificates;

pub const ROOTS: [&CStr; 2] = [c"/etc/ssl/roots.pem", c"/media/host/roots.pem"];
const ROOTS_PIECE: usize = 4096;
const CERTIFICATE_BYTES: usize = 8192;

const USAGE: &[u8] = b"usage: fetch [-q] [-o <file>] <url> [<file>]";
const USER_AGENT: &[u8] = b"EggOS-fetch";
const MODE: u32 = 0o644;
pub const CONNECT_TIMEOUT_MS: u32 = 10_000;
pub const RECEIVE_TIMEOUT_MS: u32 = 30_000;
pub const MAX_REDIRECTS: usize = 5;
pub const MAX_URL_BYTES: usize = 2048;
const REQUEST_BYTES: usize = MAX_URL_BYTES + 512;
const PIECE: usize = 4096;

struct Address {
    bytes: [u8; MAX_URL_BYTES],
    length: usize,
}

impl Address {
    fn of(parts: &[&[u8]]) -> Option<Self> {
        let mut address = Self {
            bytes: [0; MAX_URL_BYTES],
            length: 0,
        };
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
    sink: Sink<'a>,
}

enum Answer {
    Saved,
    Redirect(Address),
}

pub fn fetch<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut quiet = false;
    let mut file: Option<&CStr> = None;
    let mut operands: [Option<&CStr>; 2] = [None; 2];
    let mut count = 0;
    let mut options_end = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index];
        index += 1;
        match argument.to_bytes() {
            b"--" if !options_end => options_end = true,
            b"-q" if !options_end => quiet = true,
            b"-o" if !options_end => match arguments.get(index) {
                Some(&path) => {
                    file = Some(path);
                    index += 1;
                }
                None => {
                    output.error(&[b"option requires an argument -- 'o'"]);
                    return FAILURE;
                }
            },
            option if !options_end && option.starts_with(b"-") && option.len() > 1 => {
                return output.invalid_option(option);
            }
            _ if count < operands.len() => {
                operands[count] = Some(argument);
                count += 1;
            }
            _ => {
                output.error(&[USAGE]);
                return FAILURE;
            }
        }
    }
    let (url, file) = match (operands, file) {
        ([Some(url), None], file) => (url, file),
        ([Some(url), Some(path)], None) => (url, Some(path)),
        _ => {
            output.error(&[USAGE]);
            return FAILURE;
        }
    };
    let sink = match file {
        Some(path) if path.to_bytes() != b"-" => Sink::File(path),
        _ => Sink::StandardOutput,
    };
    let request = Request { quiet, sink };
    let Some(mut address) = Address::of(&[url.to_bytes()]) else {
        output.error(&[b"the URL is longer than 2048 bytes"]);
        return FAILURE;
    };
    for _ in 0..=MAX_REDIRECTS {
        let url = match http::parse_url(address.as_bytes()) {
            Ok(url) => url,
            Err(error) => {
                output.error(&[address.as_bytes(), b": ", error.description().as_bytes()]);
                return FAILURE;
            }
        };
        match get(output, &url, &request) {
            Ok(Answer::Saved) => return 0,
            Ok(Answer::Redirect(next)) => address = next,
            Err(status) => return status,
        }
    }
    output.error(&[url.to_bytes(), b": more than 5 redirects"]);
    FAILURE
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

struct RootsFile<'r, 'o, S: System> {
    output: &'r RefCell<&'o mut Output<S>>,
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

fn tls_failure<S: System>(output: &mut Output<S>, shown: &Shown<264>, error: client::Error) -> i32 {
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
    let answer = exchange(output, descriptor, address, url, request, buffers.as_mut());
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
) -> Result<Answer, i32> {
    let shown = authority(url);
    let connected = output
        .system
        .connect(descriptor, address, url.port, CONNECT_TIMEOUT_MS);
    if let Err(errno) = connected {
        output.error(&[shown.as_bytes(), b": ", errno.message()]);
        return Err(FAILURE);
    }
    let mut link = Link {
        descriptor,
        session: None,
    };
    if let Some(buffers) = buffers {
        link.session = Some(handshake(output, descriptor, url, &shown, buffers)?);
    }
    let mut text = [0u8; REQUEST_BYTES];
    let Some(length) = http::write_request(&mut text, url, USER_AGENT) else {
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
                output.error(&[location, b": ", error.description().as_bytes()]);
                return Err(FAILURE);
            }
        };
        return match next {
            Some(next) => Ok(Answer::Redirect(next)),
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
            parsed.reason,
        ]);
        return Err(FAILURE);
    }
    let mut body = Body::new(parsed.framing);
    let sink = match request.sink {
        Sink::StandardOutput => None,
        Sink::File(path) => match output.system.create(path, MODE, true) {
            Ok(file) => Some((file, path)),
            Err(errno) => {
                output.file_error(path, errno);
                return Err(FAILURE);
            }
        },
    };
    let mut reason = [0u8; 64];
    let reason_length = parsed.reason.len().min(reason.len());
    reason[..reason_length].copy_from_slice(&parsed.reason[..reason_length]);
    let leftover_start = head_length;
    let saved = save(
        output,
        &mut link,
        &mut body,
        &head[leftover_start..filled],
        sink,
        &shown,
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
        if reason_length > 0 {
            output.write(b" ");
            output.write(&reason[..reason_length]);
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
) -> Result<u64, i32> {
    let mut saved = feed(output, body, first, sink, shown)?;
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
    }
    Ok(saved)
}

#[cfg(test)]
mod tests;
