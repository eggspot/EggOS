//! wininet (D178, decided by research): `InternetOpenUrlW` fetches an `http://` URL with an
//! HTTP/1.1 `GET` over a TCP socket, as EggOS's `fetch` does with the same protocol code
//! (`eggos_net::http`, D158): the host resolves as wsock32's names do, redirects are followed
//! (at most five) unless the program says not to, and `InternetReadFile` hands out the body
//! decoded (by its length, in chunks, or to the connection's close). `https://` runs the same
//! over EggOS's TLS 1.3 client (D352), the server's certificate checked against the machine's
//! roots; there are no proxies, other schemes or asynchronous handles.

use super::{Bool, FALSE, SpinLock, TRUE, error, fail, heap};
use crate::library::Export;
use crate::linux::{self, AF_INET, MSG_NOSIGNAL};
use crate::text;
use eggos_net::http::{self, Body, Framing, Redirect};
use eggos_tls::chain::{NoRoots, TrustAnchors};
use eggos_tls::client::{self, Buffers, RANDOMNESS_BYTES, Session, Settings, Stopped, Transport};
use eggos_tls::pem::Certificates;

const ERROR_INTERNET_TIMEOUT: u32 = 12002;
const ERROR_INTERNET_INVALID_URL: u32 = 12005;
const ERROR_INTERNET_UNRECOGNIZED_SCHEME: u32 = 12006;
const ERROR_INTERNET_NAME_NOT_RESOLVED: u32 = 12007;
const ERROR_INTERNET_INVALID_CA: u32 = 12045;
const ERROR_INTERNET_CANNOT_CONNECT: u32 = 12029;
const ERROR_INTERNET_CONNECTION_ABORTED: u32 = 12030;
const ERROR_HTTP_INVALID_SERVER_RESPONSE: u32 = 12152;
const ERROR_HTTP_REDIRECT_FAILED: u32 = 12156;
const ERROR_INTERNET_SECURE_CHANNEL_ERROR: u32 = 12157;
const INTERNET_OPEN_TYPE_PROXY: u32 = 3;
const INTERNET_FLAG_ASYNC: u32 = 0x1000_0000;
const INTERNET_FLAG_NO_AUTO_REDIRECT: u32 = 0x0020_0000;
const MAX_HANDLES: usize = 64;
const MAX_REDIRECTS: usize = 5;
const URL_CAPACITY: usize = 2048;
const AGENT_CAPACITY: usize = 256;
const REQUEST_CAPACITY: usize = 4096;
const TIMEOUT_NANOSECONDS: u64 = 30_000_000_000;
const EINTR: u16 = 4;
const SOCK_STREAM: usize = 1;
const BUFFERS_DATA: usize = 32;
const BUFFERS_LENGTH: usize = 40;

struct Session {
    agent: [u8; AGENT_CAPACITY],
    agent_length: usize,
}

struct Request {
    link: Link,
    body: Body,
    ended: bool,
    ready: (usize, usize),
    raw: (usize, usize),
    buffer: [u8; http::MAX_HEAD_BYTES],
}

#[allow(clippy::large_enum_variant)]
enum Internet {
    Session(Session),
    Request(Request),
}

static HANDLES: SpinLock<[usize; MAX_HANDLES]> = SpinLock::new([0; MAX_HANDLES]);

fn register(internet: Internet) -> usize {
    let block = heap::allocate(core::mem::size_of::<Internet>()).cast::<Internet>();
    if block.is_null() {
        if let Internet::Request(mut request) = internet {
            request.link.close();
        }
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    }
    // SAFETY: a fresh, 16-byte aligned heap block for one `Internet`.
    unsafe { block.write(internet) };
    let registered = HANDLES.with(|handles| {
        let slot = handles.iter_mut().find(|slot| **slot == 0)?;
        *slot = block as usize;
        Some(())
    });
    match registered {
        Some(()) => block as usize,
        None => {
            release(block);
            fail(error::NOT_ENOUGH_MEMORY, 0)
        }
    }
}

fn release(block: *mut Internet) {
    // SAFETY: the block is a live `Internet` no table names any more, used by no one else.
    if let Internet::Request(request) = unsafe { &mut *block } {
        request.link.close();
    }
    // SAFETY: as above.
    unsafe { heap::free(block.cast()) };
}

fn is_handle(handle: usize) -> bool {
    handle != 0 && HANDLES.with(|handles| handles.contains(&handle))
}

unsafe fn session_agent<'a>(handle: usize) -> Option<&'a [u8]> {
    if !is_handle(handle) {
        return None;
    }
    // SAFETY: a registered handle is a live `Internet`, which a session's caller uses alone.
    match unsafe { &*(handle as *const Internet) } {
        Internet::Session(session) => Some(&session.agent[..session.agent_length]),
        Internet::Request(_) => None,
    }
}

unsafe extern "efiapi" fn internet_open_w(
    agent: *const u16,
    access: u32,
    _proxy: *const u16,
    _bypass: *const u16,
    flags: u32,
) -> usize {
    if access == INTERNET_OPEN_TYPE_PROXY || flags & INTERNET_FLAG_ASYNC != 0 {
        return fail(error::NOT_SUPPORTED, 0);
    }
    let mut session = Session {
        agent: [0; AGENT_CAPACITY],
        agent_length: 0,
    };
    if !agent.is_null() {
        // SAFETY: `InternetOpenW`'s contract: a NUL-terminated agent name.
        let agent = unsafe { super::file::wide_string(agent) };
        session.agent_length = text::write_utf8(agent, &mut session.agent).0;
    }
    register(Internet::Session(session))
}

fn send_all(fd: usize, mut bytes: &[u8]) -> Result<(), u32> {
    while !bytes.is_empty() {
        match linux::send(fd, bytes, MSG_NOSIGNAL) {
            Ok(0) | Err(_) => return Err(ERROR_INTERNET_CONNECTION_ABORTED),
            Ok(sent) => bytes = &bytes[sent.min(bytes.len())..],
        }
    }
    Ok(())
}

fn receive_some(fd: usize, buffer: &mut [u8]) -> Result<usize, u32> {
    loop {
        match linux::wait_readable([fd as i32, -1], Some(TIMEOUT_NANOSECONDS)) {
            Ok(0) => return Err(ERROR_INTERNET_TIMEOUT),
            Err(errno) if errno.0 == EINTR => continue,
            _ => break,
        }
    }
    linux::receive(fd, buffer, 0).map_err(|_| ERROR_INTERNET_CONNECTION_ABORTED)
}

const ROOTS: [&[u8]; 2] = [b"/etc/ssl/roots.pem\0", b"/media/host/roots.pem\0"];
const ROOTS_PIECE: usize = 4096;
const CERTIFICATE_BYTES: usize = 8192;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

struct Socket {
    fd: usize,
}

impl Transport for Socket {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        send_all(self.fd, bytes).map_err(|_| Stopped)
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        receive_some(self.fd, buffer).map_err(|_| Stopped)
    }
}

struct RootsFile;

impl TrustAnchors for RootsFile {
    fn each(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots> {
        let fd = ROOTS
            .iter()
            .find_map(|path| linux::open(path, linux::O_RDONLY).ok())
            .ok_or(NoRoots)?;
        let mut der = [0u8; CERTIFICATE_BYTES];
        let mut decoder = Certificates::new(&mut der);
        let mut text = [0u8; ROOTS_PIECE];
        let found = loop {
            match linux::read(fd, &mut text) {
                Ok(0) => break decoder.finish(visit),
                Ok(count) if decoder.feed(&text[..count], visit) => break true,
                Ok(_) => {}
                Err(errno) if errno.0 == EINTR => {}
                Err(_) => break false,
            }
        };
        let _ = linux::close(fd);
        Ok(found)
    }
}

/// An `https://` connection's TLS session, over buffers in their own heap block, which the
/// session borrows until the link closes.
struct Secure {
    session: Session<'static>,
    buffers: *mut Buffers,
}

/// A request's connection: plain TCP for `http://`, TLS over it for `https://` (D352).
struct Link {
    fd: usize,
    secure: Option<Secure>,
}

impl Link {
    fn plain(fd: usize) -> Link {
        Link { fd, secure: None }
    }

    /// The TLS handshake with `host` over connected `fd`, the server's certificate checked
    /// against the machine's roots; the descriptor is closed when it fails.
    fn secure(fd: usize, host: &[u8]) -> Result<Link, u32> {
        let failed = |code: u32| {
            let _ = linux::close(fd);
            Err(code)
        };
        let block = heap::allocate(core::mem::size_of::<Buffers>()).cast::<Buffers>();
        if block.is_null() {
            return failed(error::NOT_ENOUGH_MEMORY);
        }
        // SAFETY: a fresh, 16-byte aligned heap block the size of `Buffers`, freed only when
        // the link closes, after the session that borrows it.
        let buffers: &'static mut Buffers = unsafe {
            block.write(Buffers::default());
            &mut *block
        };
        let mut randomness = [0u8; RANDOMNESS_BYTES];
        if linux::random_bytes(&mut randomness).is_err() {
            // SAFETY: the block made above, which nothing borrows any more.
            unsafe { heap::free(block.cast()) };
            return failed(ERROR_INTERNET_SECURE_CHANNEL_ERROR);
        }
        let settings = Settings {
            host,
            now: (linux::clock_nanoseconds(linux::CLOCK_REALTIME) / NANOSECONDS_PER_SECOND) as i64,
        };
        let mut socket = Socket { fd };
        match client::connect(&mut socket, buffers, &settings, &randomness, &mut RootsFile) {
            Ok(session) => Ok(Link {
                fd,
                secure: Some(Secure {
                    session,
                    buffers: block,
                }),
            }),
            Err(failure) => {
                // SAFETY: the session that borrowed the block is gone with the error.
                unsafe { heap::free(block.cast()) };
                failed(tls_error(failure))
            }
        }
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), u32> {
        let mut socket = Socket { fd: self.fd };
        match self.secure.as_mut() {
            Some(secure) => secure
                .session
                .write(&mut socket, bytes)
                .map_err(|_| ERROR_INTERNET_CONNECTION_ABORTED),
            None => send_all(self.fd, bytes),
        }
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, u32> {
        let mut socket = Socket { fd: self.fd };
        match self.secure.as_mut() {
            Some(secure) => secure
                .session
                .read(&mut socket, buffer)
                .map_err(|_| ERROR_INTERNET_CONNECTION_ABORTED),
            None => receive_some(self.fd, buffer),
        }
    }

    /// Ends the session (TLS's close notice), frees its buffers and closes the descriptor.
    fn close(&mut self) {
        if let Some(mut secure) = self.secure.take() {
            let mut socket = Socket { fd: self.fd };
            let _ = secure.session.close(&mut socket);
            let block = secure.buffers;
            drop(secure);
            // SAFETY: the block `secure` made; the session that borrowed it is dropped.
            unsafe { heap::free(block.cast()) };
        }
        let _ = linux::close(self.fd);
    }
}

fn tls_error(failure: client::Error) -> u32 {
    match failure {
        client::Error::Transport => ERROR_INTERNET_CONNECTION_ABORTED,
        client::Error::Certificate(_) => ERROR_INTERNET_INVALID_CA,
        _ => ERROR_INTERNET_SECURE_CHANNEL_ERROR,
    }
}

fn connect(url: &http::Url<'_>) -> Result<usize, u32> {
    let mut addresses = [[0u8; 4]; super::winsock::MAX_ADDRESSES];
    let count = super::winsock::resolve(url.host, &mut addresses)
        .map_err(|_| ERROR_INTERNET_NAME_NOT_RESOLVED)?;
    let fd = linux::socket(AF_INET, SOCK_STREAM, 0).map_err(|_| ERROR_INTERNET_CANNOT_CONNECT)?;
    let connected = addresses[..count].iter().any(|address| {
        linux::connect(fd, &super::winsock::socket_address(*address, url.port)).is_ok()
    });
    if !connected {
        let _ = linux::close(fd);
        return Err(ERROR_INTERNET_CANNOT_CONNECT);
    }
    Ok(fd)
}

fn url_error(error: http::UrlError) -> u32 {
    match error {
        http::UrlError::NotHttp => ERROR_INTERNET_UNRECOGNIZED_SCHEME,
        _ => ERROR_INTERNET_INVALID_URL,
    }
}

struct Answer {
    framing: Framing,
    redirect: bool,
    head_length: usize,
}

fn read_head(link: &mut Link, buffer: &mut [u8], filled: &mut usize) -> Result<Answer, u32> {
    loop {
        if let Some(end) = http::head_end(&buffer[..*filled]) {
            let head =
                http::parse_head(&buffer[..end]).map_err(|_| ERROR_HTTP_INVALID_SERVER_RESPONSE)?;
            if head.is_interim() {
                buffer.copy_within(end..*filled, 0);
                *filled -= end;
                continue;
            }
            return Ok(Answer {
                framing: head.framing,
                redirect: head.is_redirect() && head.location.is_some(),
                head_length: end,
            });
        }
        if *filled == buffer.len() {
            return Err(ERROR_HTTP_INVALID_SERVER_RESPONSE);
        }
        match link.receive(&mut buffer[*filled..])? {
            0 => return Err(ERROR_HTTP_INVALID_SERVER_RESPONSE),
            read => *filled += read,
        }
    }
}

fn redirected_url(
    buffer: &[u8],
    head_length: usize,
    current: &http::Url<'_>,
    out: &mut [u8; URL_CAPACITY],
) -> Result<usize, u32> {
    let head =
        http::parse_head(&buffer[..head_length]).map_err(|_| ERROR_HTTP_INVALID_SERVER_RESPONSE)?;
    let location = head.location.ok_or(ERROR_HTTP_REDIRECT_FAILED)?;
    let mut length = 0;
    let mut put = |bytes: &[u8]| -> Result<(), u32> {
        let end = length + bytes.len();
        out.get_mut(length..end)
            .ok_or(ERROR_HTTP_REDIRECT_FAILED)?
            .copy_from_slice(bytes);
        length = end;
        Ok(())
    };
    match http::redirect(location).map_err(|_| ERROR_HTTP_REDIRECT_FAILED)? {
        Redirect::Absolute(_) => put(location)?,
        Redirect::Path(path) => {
            put(if current.secure { b"https://" } else { b"http://" })?;
            put(current.host)?;
            let mut port = [0u8; 6];
            port[0] = b':';
            let mut digits = 1;
            let mut value = current.port;
            let mut reversed = [0u8; 5];
            let mut count = 0;
            loop {
                reversed[count] = b'0' + (value % 10) as u8;
                count += 1;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            for digit in reversed[..count].iter().rev() {
                port[digits] = *digit;
                digits += 1;
            }
            put(&port[..digits])?;
            put(path)?;
        }
    }
    Ok(length)
}

fn open_url(
    agent: &[u8],
    url_text: &mut [u8; URL_CAPACITY],
    mut length: usize,
    flags: u32,
) -> Result<Request, u32> {
    let mut request = Request {
        link: Link::plain(0),
        body: Body::new(Framing::Length(0)),
        ended: false,
        ready: (0, 0),
        raw: (0, 0),
        buffer: [0; http::MAX_HEAD_BYTES],
    };
    for _ in 0..=MAX_REDIRECTS {
        let copy = *url_text;
        let url = http::parse_url(&copy[..length]).map_err(url_error)?;
        let fd = connect(&url)?;
        let mut link = match url.secure {
            true => Link::secure(fd, url.host)?,
            false => Link::plain(fd),
        };
        let mut message = [0u8; REQUEST_CAPACITY];
        let answer = http::write_request(&mut message, &url, agent)
            .ok_or(ERROR_INTERNET_INVALID_URL)
            .and_then(|size| link.send(&message[..size]))
            .and_then(|()| {
                let mut filled = 0;
                let answer = read_head(&mut link, &mut request.buffer, &mut filled)?;
                Ok((answer, filled))
            });
        let (answer, filled) = match answer {
            Ok(answer) => answer,
            Err(code) => {
                link.close();
                return Err(code);
            }
        };
        if answer.redirect && flags & INTERNET_FLAG_NO_AUTO_REDIRECT == 0 {
            link.close();
            length = redirected_url(&request.buffer, answer.head_length, &url, url_text)?;
            continue;
        }
        request.link = link;
        request.body = Body::new(answer.framing);
        request.raw = (answer.head_length, filled);
        return Ok(request);
    }
    Err(ERROR_HTTP_REDIRECT_FAILED)
}

unsafe extern "efiapi" fn internet_open_url_w(
    session: usize,
    url: *const u16,
    _headers: *const u16,
    _headers_length: u32,
    flags: u32,
    _context: usize,
) -> usize {
    // SAFETY: a session handle's agent lives as long as the handle.
    let Some(agent) = (unsafe { session_agent(session) }) else {
        return fail(error::INVALID_HANDLE, 0);
    };
    if url.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `InternetOpenUrlW`'s contract: a NUL-terminated URL.
    let url = unsafe { super::file::wide_string(url) };
    let mut url_text = [0u8; URL_CAPACITY];
    let (length, consumed) = text::write_utf8(url, &mut url_text);
    if consumed < url.len() {
        return fail(ERROR_INTERNET_INVALID_URL, 0);
    }
    match open_url(agent, &mut url_text, length, flags) {
        Ok(request) => register(Internet::Request(request)),
        Err(code) => fail(code, 0),
    }
}

fn read_body(request: &mut Request, out: &mut [u8]) -> Result<usize, u32> {
    let mut written = 0;
    while written < out.len() {
        let (start, end) = request.ready;
        if start < end {
            let count = (end - start).min(out.len() - written);
            out[written..written + count].copy_from_slice(&request.buffer[start..start + count]);
            written += count;
            request.ready.0 += count;
            continue;
        }
        if request.ended || request.body.is_done() {
            break;
        }
        let (raw_start, raw_end) = request.raw;
        if raw_start < raw_end {
            let input = &request.buffer[raw_start..raw_end];
            let decoded = request
                .body
                .decode(input)
                .map_err(|_| ERROR_HTTP_INVALID_SERVER_RESPONSE)?;
            request.ready = match decoded.data.is_empty() {
                true => (0, 0),
                false => {
                    let start = decoded.data.as_ptr() as usize - request.buffer.as_ptr() as usize;
                    (start, start + decoded.data.len())
                }
            };
            request.raw.0 += decoded.used;
            continue;
        }
        request.ready = (0, 0);
        request.raw = (0, 0);
        match request.link.receive(&mut request.buffer)? {
            0 => {
                request.ended = true;
                request
                    .body
                    .finish()
                    .map_err(|_| ERROR_INTERNET_CONNECTION_ABORTED)?;
            }
            read => request.raw = (0, read),
        }
    }
    Ok(written)
}

unsafe fn request_of<'a>(handle: usize) -> Option<&'a mut Request> {
    if !is_handle(handle) {
        return None;
    }
    // SAFETY: a registered handle is a live `Internet`; a request handle is read by one thread
    // at a time, as WinINet's synchronous handles are.
    match unsafe { &mut *(handle as *mut Internet) } {
        Internet::Request(request) => Some(request),
        Internet::Session(_) => None,
    }
}

unsafe fn read_into(handle: usize, buffer: *mut u8, size: u32) -> Result<usize, u32> {
    // SAFETY: the callers' contract: a request handle.
    let request = unsafe { request_of(handle) }.ok_or(error::INVALID_HANDLE)?;
    if size == 0 {
        return Ok(0);
    }
    if buffer.is_null() {
        return Err(error::INVALID_PARAMETER);
    }
    // SAFETY: the callers' contract: `size` writable bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) };
    read_body(request, out)
}

unsafe extern "efiapi" fn internet_read_file(
    handle: usize,
    buffer: *mut u8,
    size: u32,
    read: *mut u32,
) -> Bool {
    if read.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `InternetReadFile`'s contract: `size` writable bytes at `buffer`.
    match unsafe { read_into(handle, buffer, size) } {
        Ok(count) => {
            // SAFETY: the caller passes a writable `DWORD`.
            unsafe { read.write(count as u32) };
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn internet_read_file_ex_a(
    handle: usize,
    buffers: *mut u8,
    _flags: u32,
    _context: usize,
) -> Bool {
    if buffers.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let data_field = buffers.wrapping_add(BUFFERS_DATA).cast::<*mut u8>();
    let length_field = buffers.wrapping_add(BUFFERS_LENGTH).cast::<u32>();
    // SAFETY: `InternetReadFileExA`'s contract: a readable and writable `INTERNET_BUFFERSA`.
    let data = unsafe { data_field.read_unaligned() };
    // SAFETY: as above.
    let size = unsafe { length_field.read_unaligned() };
    // SAFETY: as above: `lpvBuffer` holds `dwBufferLength` writable bytes.
    match unsafe { read_into(handle, data, size) } {
        Ok(count) => {
            // SAFETY: as above.
            unsafe { length_field.write_unaligned(count as u32) };
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn internet_close_handle(handle: usize) -> Bool {
    let removed = HANDLES.with(|handles| {
        let slot = handles
            .iter_mut()
            .find(|slot| **slot == handle && handle != 0)?;
        *slot = 0;
        Some(())
    });
    match removed {
        Some(()) => {
            release(handle as *mut Internet);
            TRUE
        }
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"InternetOpenW" => internet_open_w,
    b"InternetOpenUrlW" => internet_open_url_w,
    b"InternetReadFile" => internet_read_file,
    b"InternetReadFileExA" => internet_read_file_ex_a,
    b"InternetCloseHandle" => internet_close_handle,
];
