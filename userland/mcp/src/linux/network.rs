//! `eggmcpd --network` (D193, D576): MCP over HTTPS for paired agents on other machines. Init
//! starts it only once the person turned it on (`agent network on`, which writes
//! [`NETWORK_ENABLED`]). It takes up to [`MAX_CONNECTIONS`] connections at once on
//! [`http::DEFAULT_PORT`], at most [`MAX_CONNECTIONS_PER_ADDRESS`] from one address, each in a
//! process of its own: a TLS handshake with EggOS's own certificate, then HTTP requests until
//! the client closes or stays idle for [`IDLE_MS`], or the session's event stream until the
//! client goes. The MCP messages go to this process ([`relay`]), where each paired agent has
//! a server of its own and each session opened by `initialize` another, pinned to that agent,
//! with no bound but the person's grants. The process changes no grant: it reads the store
//! before every request, and only counts tries in, or removes, the pairing file.

mod advertise;
mod relay;

use super::{
    AT_FDCWD, BUFFERS, Buffers, Linux, O_CREAT, O_EXCL, O_RDONLY, O_WRONLY, POLLERR, POLLHUP,
    POLLIN, PollDescriptor, call, close, ignore_broken_pipes, now_ms, open, pause, rename, say,
};
use crate::grants::{AgentId, GrantStore, Grants, STORE_BYTES, StoreState, Unknown};
use crate::http::{self, Parsed, Route, Status};
use crate::pairing::{
    Attempt, NETWORK_CERTIFICATE, NETWORK_CERTIFICATE_PEM, NETWORK_ENABLED, NETWORK_KEY,
    PAIRING_BYTES, PAIRING_PATH, PAIRING_TEMPORARY_PATH, PendingPairing, WRONG_CODE_PAUSE_MS,
};
use crate::server::{Scratch, Server};
use crate::system::System;
use core::ffi::CStr;
use eggos_crypto::p256::{self, SCALAR_BYTES};
use eggos_net::status::parse_interfaces;
use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::{self, number};
use eggos_tls::certificate::{self, MAX_CERTIFICATE_BYTES, Name};
use eggos_tls::client::{self as records, Session, Stopped, Transport};
use eggos_tls::server::{self as tls_server, Identity};

/// The word that starts the network server.
pub const NETWORK_OPTION: &[u8] = b"--network";
/// How long a connection may stay silent, in milliseconds.
pub const IDLE_MS: u64 = 30_000;
/// How long a TLS handshake may take, in milliseconds.
pub const HANDSHAKE_MS: u64 = 10_000;
/// How long a request may take to arrive once it started, in milliseconds, however its bytes
/// are spread over records.
pub const REQUEST_MS: u64 = 10_000;
/// The years the certificate is valid for.
pub const CERTIFICATE_YEARS: i64 = 20;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOCK_CLOEXEC: usize = 0o2_000_000;
const SOL_SOCKET: usize = 1;
const SO_REUSEADDR: usize = 2;
const BACKLOG: usize = 8;
const KEY_MODE: usize = 0o600;
const PUBLIC_MODE: usize = 0o644;
const CERTIFICATE_START: i64 = 1_767_225_600;
const INTERFACES: &CStr = c"/proc/net/interfaces";
const HOSTNAME: &CStr = c"/proc/sys/kernel/hostname";
const LOCAL_NAME: &[u8] = b"eggos.local";
const MAX_SERVERS: usize = crate::grants::MAX_AGENTS;
const AF_UNIX: usize = 1;
const SESSION_ID_BYTES: usize = 32;

/// The most connections at once.
pub const MAX_CONNECTIONS: usize = 16;
/// The most connections at once from one address.
pub const MAX_CONNECTIONS_PER_ADDRESS: usize = 4;
/// The most sessions kept; opening one more ends the one unused longest.
pub const MAX_SESSIONS: usize = 16;
/// How often an idle event stream gets a keep-alive, in milliseconds.
pub const STREAM_KEEP_ALIVE_MS: u64 = 15_000;

struct Socket {
    descriptor: usize,
    deadline_ms: u64,
}

impl Transport for Socket {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        match syscall::write_all(self.descriptor, bytes) {
            true => Ok(()),
            false => Err(Stopped),
        }
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        loop {
            let left = self.deadline_ms.saturating_sub(now_ms());
            if left == 0 {
                return Err(Stopped);
            }
            let mut watched = [PollDescriptor {
                descriptor: self.descriptor as i32,
                events: POLLIN,
                returned_events: 0,
            }];
            let timeout = [(left / 1000) as i64, (left % 1000 * 1_000_000) as i64];
            match call(
                number::PPOLL,
                [
                    watched.as_mut_ptr() as usize,
                    1,
                    timeout.as_ptr() as usize,
                    0,
                    8,
                    0,
                ],
            ) {
                Ok(0) => return Err(Stopped),
                Err(Errno::EINTR) => continue,
                Err(_) => return Err(Stopped),
                Ok(_) => {}
            }
            match call(
                number::READ,
                [
                    self.descriptor,
                    buffer.as_mut_ptr() as usize,
                    buffer.len(),
                    0,
                    0,
                    0,
                ],
            ) {
                Ok(count) => return Ok(count),
                Err(Errno::EINTR | Errno::EAGAIN) => continue,
                Err(_) if watched[0].returned_events & (POLLHUP | POLLERR) != 0 => return Ok(0),
                Err(_) => return Err(Stopped),
            }
        }
    }
}

fn random<const N: usize>() -> Option<[u8; N]> {
    let mut bytes = [0u8; N];
    let mut filled = 0;
    while filled < N {
        let free = bytes.get_mut(filled..)?;
        match call(
            number::GETRANDOM,
            [free.as_mut_ptr() as usize, free.len(), 0, 0, 0, 0],
        ) {
            Ok(count) => filled += count,
            Err(Errno::EINTR) => continue,
            Err(_) => return None,
        }
    }
    Some(bytes)
}

fn read_c_file(path: &CStr, out: &mut [u8]) -> Result<usize, Errno> {
    let file = open(path, O_RDONLY, 0)?;
    let mut length = 0;
    let result = loop {
        let Some(free) = out.get_mut(length..).filter(|free| !free.is_empty()) else {
            break Ok(length);
        };
        match call(
            number::READ,
            [file, free.as_mut_ptr() as usize, free.len(), 0, 0, 0],
        ) {
            Ok(0) => break Ok(length),
            Ok(count) => length += count,
            Err(Errno::EINTR) => continue,
            Err(error) => break Err(error),
        }
    };
    close(file);
    result
}

fn write_file(target: &CStr, spare: &CStr, mode: usize, bytes: &[u8]) -> Result<(), Errno> {
    let _ = call(
        number::UNLINKAT,
        [AT_FDCWD, spare.as_ptr() as usize, 0, 0, 0, 0],
    );
    let file = open(spare, O_WRONLY | O_CREAT | O_EXCL, mode)?;
    let written = syscall::write_all(file, bytes);
    let synced = call(number::FSYNC, [file, 0, 0, 0, 0, 0]);
    close(file);
    if !written {
        return Err(Errno::EIO);
    }
    synced?;
    rename(spare, target)
}

fn remove(path: &CStr) {
    let _ = call(
        number::UNLINKAT,
        [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0],
    );
}

fn private_key() -> Option<[u8; SCALAR_BYTES]> {
    let mut key = [0u8; SCALAR_BYTES + 1];
    if let Ok(SCALAR_BYTES) = read_c_file(NETWORK_KEY, &mut key) {
        let mut kept = [0u8; SCALAR_BYTES];
        kept.copy_from_slice(&key[..SCALAR_BYTES]);
        return p256::public_key(&kept).ok().map(|_| kept);
    }
    let key = loop {
        if let Some(key) = p256::private_key_from(&random()?) {
            break key;
        }
    };
    write_file(
        NETWORK_KEY,
        c"/var/lib/eggos/agent-network.key.new",
        KEY_MODE,
        &key,
    )
    .ok()?;
    Some(key)
}

fn host_name(out: &mut [u8; certificate::MAX_HOST_BYTES]) -> usize {
    let mut text = [0u8; 80];
    let length = read_c_file(HOSTNAME, &mut text).unwrap_or(0);
    let name = text.get(..length).unwrap_or_default();
    let name = name.strip_suffix(b"\n").unwrap_or(name);
    let fits = !name.is_empty()
        && name.len() <= out.len()
        && name
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    match fits {
        true => {
            out[..name.len()].copy_from_slice(name);
            name.len()
        }
        false => 0,
    }
}

fn addresses() -> ([[u8; 4]; 4], usize) {
    let mut text = [0u8; 4096];
    let length = read_c_file(INTERFACES, &mut text).unwrap_or(0);
    let (interfaces, count) = parse_interfaces::<8>(text.get(..length).unwrap_or_default());
    let mut found = [[0u8; 4]; 4];
    let mut kept = 0;
    for interface in interfaces.iter().take(count).flatten() {
        if let Some(subnet) = interface.subnet
            && subnet.address.0 != [127, 0, 0, 1]
            && let Some(slot) = found.get_mut(kept)
        {
            *slot = subnet.address.0;
            kept += 1;
        }
    }
    (found, kept)
}

fn make_certificate(key: &[u8; SCALAR_BYTES], out: &mut [u8; MAX_CERTIFICATE_BYTES]) -> usize {
    let mut host = [0u8; certificate::MAX_HOST_BYTES];
    let host_length = host_name(&mut host);
    let (found, count) = addresses();
    let mut names = [Name::Host(LOCAL_NAME); certificate::MAX_NAMES];
    let mut used = 1;
    if host_length > 0 && &host[..host_length] != LOCAL_NAME {
        names[used] = Name::Host(&host[..host_length]);
        used += 1;
    }
    for address in found.iter().take(count) {
        if let Some(slot) = names.get_mut(used) {
            *slot = Name::Ipv4(*address);
            used += 1;
        }
    }
    let mut serial = [0u8; certificate::SERIAL_BYTES];
    let public_key = p256::public_key(key).unwrap_or([0; p256::PUBLIC_KEY_BYTES]);
    serial.copy_from_slice(&certificate::fingerprint(&public_key)[..certificate::SERIAL_BYTES]);
    let request = certificate::Request {
        private_key: key,
        names: &names[..used],
        serial,
        not_before: CERTIFICATE_START,
        not_after: CERTIFICATE_START + CERTIFICATE_YEARS * 365 * 86_400,
    };
    certificate::self_signed(&request, out).unwrap_or(0)
}

fn load_identity(
    key: &[u8; SCALAR_BYTES],
    certificate_out: &mut [u8; MAX_CERTIFICATE_BYTES],
) -> usize {
    if let Ok(length) = read_c_file(NETWORK_CERTIFICATE, certificate_out)
        && length > 0
        && length < MAX_CERTIFICATE_BYTES
    {
        return length;
    }
    let length = make_certificate(key, certificate_out);
    let der = certificate_out.get(..length).unwrap_or_default();
    let mut pem = [0u8; certificate::pem_bytes(MAX_CERTIFICATE_BYTES)];
    let pem_length = certificate::pem(der, &mut pem).unwrap_or(0);
    let stored = write_file(
        NETWORK_CERTIFICATE,
        c"/var/lib/eggos/agent-network.der.new",
        PUBLIC_MODE,
        der,
    )
    .and_then(|()| {
        write_file(
            NETWORK_CERTIFICATE_PEM,
            c"/var/lib/eggos/agent-network.pem.new",
            PUBLIC_MODE,
            pem.get(..pem_length).unwrap_or_default(),
        )
    });
    if stored.is_err() {
        say(&[b"eggmcpd: the network certificate could not be kept\n"]);
    }
    length
}

fn port() -> u16 {
    let mut text = [0u8; 16];
    let length = read_c_file(NETWORK_ENABLED, &mut text).unwrap_or(0);
    let text = text.get(..length).unwrap_or_default();
    let text = text.strip_suffix(b"\n").unwrap_or(text);
    crate::grants::seconds(text)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|&port| port > 0)
        .unwrap_or(http::DEFAULT_PORT)
}

fn listen(port: u16) -> Result<usize, Errno> {
    let socket = call(
        number::SOCKET,
        [AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0, 0, 0, 0],
    )?;
    let on = 1i32;
    let _ = call(
        number::SETSOCKOPT,
        [
            socket,
            SOL_SOCKET,
            SO_REUSEADDR,
            (&raw const on) as usize,
            4,
            0,
        ],
    );
    let mut address = [0u8; 16];
    address[..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
    address[2..4].copy_from_slice(&port.to_be_bytes());
    let bound = call(
        number::BIND,
        [socket, address.as_ptr() as usize, address.len(), 0, 0, 0],
    )
    .and_then(|_| call(number::LISTEN, [socket, BACKLOG, 0, 0, 0, 0]));
    match bound {
        Ok(_) => Ok(socket),
        Err(error) => {
            close(socket);
            Err(error)
        }
    }
}

struct Servers {
    slots: [Option<Server>; MAX_SERVERS],
}

impl Servers {
    fn for_agent(
        &mut self,
        agent: AgentId,
        system: &mut Linux,
        scratch: &mut Scratch<'_>,
    ) -> Option<&mut Server> {
        let index = self
            .slots
            .iter()
            .position(|slot| matches!(slot, Some(server) if server.agent() == agent))
            .or_else(|| self.slots.iter().position(Option::is_none))?;
        let slot = self.slots.get_mut(index)?;
        if slot.is_none() {
            let mut server = Server::new();
            server.set_grants(Grants::ALL, Unknown::default());
            server.set_agent(agent);
            server.set_answers_only();
            server.start(false, system, scratch);
            *slot = Some(server);
        }
        slot.as_mut()
    }

    fn forget_missing(&mut self, store: &GrantStore) {
        for slot in &mut self.slots {
            if let Some(server) = slot
                && store.get(&server.agent()).is_none()
            {
                *slot = None;
            }
        }
    }
}

fn read_store(system: &mut Linux) -> StoreState {
    let mut file = [0u8; STORE_BYTES];
    let read = system.read_grant_store(&mut file);
    StoreState::from_read(read.map(|length| file.get(..length).unwrap_or_default()))
}

fn pair(body: &[u8], now: u64, answer: &mut [u8]) -> (Status, usize) {
    let mut code = [0u8; 32];
    let Some(code) = http::pair_code(body, &mut code) else {
        return (
            Status::BadRequest,
            http::error_body("send {\"code\": \"ABCD-EFGH\"}", answer),
        );
    };
    let mut file = [0u8; PAIRING_BYTES];
    let pending = read_c_file(PAIRING_PATH, &mut file)
        .ok()
        .and_then(|length| PendingPairing::decode(file.get(..length)?).ok());
    let Some(mut pending) = pending else {
        return (
            Status::NotFound,
            http::error_body("no pairing is waiting: run agent pair on EggOS", answer),
        );
    };
    let outcome = pending.attempt(code, now);
    if let Attempt::Paired(agent, token) = outcome {
        remove(PAIRING_PATH);
        return (
            Status::Ok,
            http::pair_answer(agent.as_bytes(), &token, answer),
        );
    }
    let mut out = [0u8; PAIRING_BYTES];
    if let Ok(length) = pending.encode(&mut out) {
        let _ = write_file(
            PAIRING_PATH,
            PAIRING_TEMPORARY_PATH,
            KEY_MODE,
            out.get(..length).unwrap_or_default(),
        );
    }
    match outcome {
        Attempt::Wrong(_) | Attempt::Burned => pause(WRONG_CODE_PAUSE_MS),
        _ => {}
    }
    match outcome {
        Attempt::Wrong(_) => (
            Status::Forbidden,
            http::error_body("that code is wrong", answer),
        ),
        Attempt::Burned => (
            Status::TooManyRequests,
            http::error_body(
                "too many wrong codes: run agent pair on EggOS again",
                answer,
            ),
        ),
        _ => (
            Status::NotFound,
            http::error_body("no pairing is waiting: run agent pair on EggOS", answer),
        ),
    }
}

struct OpenSession {
    id: [u8; SESSION_ID_BYTES],
    agent: AgentId,
    server: Server,
    used_ms: u64,
    listener: Option<usize>,
}

struct Sessions {
    slots: [Option<OpenSession>; MAX_SESSIONS],
}

impl Sessions {
    fn find(&self, id: &[u8], agent: AgentId) -> Option<usize> {
        self.slots.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|open| open.id.as_slice() == id && open.agent == agent)
        })
    }

    fn open(
        &mut self,
        id: [u8; SESSION_ID_BYTES],
        agent: AgentId,
        now: u64,
        system: &mut Linux,
        scratch: &mut Scratch<'_>,
    ) -> usize {
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .or_else(|| {
                (0..MAX_SESSIONS).min_by_key(|&index| {
                    self.slots[index].as_ref().map_or(0, |open| open.used_ms)
                })
            })
            .unwrap_or(0);
        let mut server = Server::new();
        server.set_grants(Grants::ALL, Unknown::default());
        server.set_agent(agent);
        server.set_answers_only();
        server.start(false, system, scratch);
        self.slots[index] = Some(OpenSession {
            id,
            agent,
            server,
            used_ms: now,
            listener: None,
        });
        index
    }

    fn forget_missing(&mut self, store: &GrantStore) {
        for slot in &mut self.slots {
            if let Some(open) = slot
                && store.get(&open.agent).is_none()
            {
                *slot = None;
            }
        }
    }

    fn forget_listener(&mut self, connection: usize) {
        for open in self.slots.iter_mut().flatten() {
            if open.listener == Some(connection) {
                open.listener = None;
            }
        }
    }
}

fn new_session_id() -> Option<[u8; SESSION_ID_BYTES]> {
    let random = random::<{ SESSION_ID_BYTES / 2 }>()?;
    let mut id = [0u8; SESSION_ID_BYTES];
    for (index, byte) in random.iter().enumerate() {
        id[2 * index] = b"0123456789abcdef"[usize::from(byte >> 4)];
        id[2 * index + 1] = b"0123456789abcdef"[usize::from(byte & 15)];
    }
    Some(id)
}

struct Exchange<'b> {
    system: Linux,
    scratch: Scratch<'b>,
    response: &'b mut [u8],
    servers: Servers,
    sessions: Sessions,
}

struct Outcome {
    status: Status,
    start: usize,
    end: usize,
    opened: Option<[u8; SESSION_ID_BYTES]>,
    listener: Option<usize>,
}

impl Outcome {
    fn refused(status: Status) -> Self {
        Self {
            status,
            start: 0,
            end: 0,
            opened: None,
            listener: None,
        }
    }
}

impl Exchange<'_> {
    fn agent(&mut self, token_hash: &[u8; 32]) -> Option<AgentId> {
        let StoreState::Loaded(store) = read_store(&mut self.system) else {
            return None;
        };
        self.servers.forget_missing(&store);
        self.sessions.forget_missing(&store);
        store.agent_with_token(token_hash).map(|agent| *agent.id())
    }

    fn mcp(&mut self, token_hash: &[u8; 32], session: Option<&[u8]>, body: &[u8]) -> Outcome {
        let Some(agent) = self.agent(token_hash) else {
            return Outcome::refused(Status::Unauthorized);
        };
        let now = now_ms();
        let Exchange {
            system,
            scratch,
            response,
            servers,
            sessions,
        } = self;
        let mut opened = None;
        let mut listener = None;
        let server = match session {
            Some(id) => {
                let Some(open) = sessions
                    .find(id, agent)
                    .and_then(|index| sessions.slots[index].as_mut())
                else {
                    return Outcome::refused(Status::NotFound);
                };
                open.used_ms = now;
                listener = open.listener;
                &mut open.server
            }
            None if http::is_initialize(body) => {
                let Some(id) = new_session_id() else {
                    return Outcome::refused(Status::TooManyRequests);
                };
                let index = sessions.open(id, agent, now, system, scratch);
                opened = Some(id);
                let Some(open) = sessions.slots[index].as_mut() else {
                    return Outcome::refused(Status::TooManyRequests);
                };
                &mut open.server
            }
            None => {
                let Some(server) = servers.for_agent(agent, system, scratch) else {
                    return Outcome::refused(Status::Unauthorized);
                };
                server
            }
        };
        let mut length = server.handle_line(body, system, scratch, response);
        while let Some(deadline) = server.waiting_until() {
            let answer = match system.prompt_answer() {
                Some(descriptor) if wait_readable(descriptor, deadline) => {
                    Some(system.read_prompt_answer())
                }
                _ => None,
            };
            length = match answer {
                Some(answer) => server.prompt_answered(answer, system, scratch, response),
                None => server.expire(now_ms(), system, scratch, response),
            };
        }
        let lines = response.get(..length).unwrap_or_default();
        let lines = lines.strip_suffix(b"\n").unwrap_or(lines);
        let start = lines
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |newline| newline + 1);
        Outcome {
            status: match lines.is_empty() {
                true => Status::Accepted,
                false => Status::Ok,
            },
            start,
            end: lines.len(),
            opened,
            listener,
        }
    }

    fn listen(&mut self, token_hash: &[u8; 32], session: &[u8], connection: usize) -> Status {
        let Some(agent) = self.agent(token_hash) else {
            return Status::Unauthorized;
        };
        match self.sessions.find(session, agent) {
            Some(index) => {
                if let Some(open) = self.sessions.slots[index].as_mut() {
                    open.listener = Some(connection);
                }
                Status::Ok
            }
            None => Status::NotFound,
        }
    }

    fn end_session(&mut self, token_hash: &[u8; 32], session: &[u8]) -> Status {
        let Some(agent) = self.agent(token_hash) else {
            return Status::Unauthorized;
        };
        match self.sessions.find(session, agent) {
            Some(index) => {
                self.sessions.slots[index] = None;
                Status::Ok
            }
            None => Status::NotFound,
        }
    }
}

fn wait_readable(descriptor: usize, deadline_ms: u64) -> bool {
    let left = deadline_ms.saturating_sub(now_ms());
    let mut watched = [PollDescriptor {
        descriptor: descriptor as i32,
        events: POLLIN,
        returned_events: 0,
    }];
    let timeout = [(left / 1000) as i64, (left % 1000 * 1_000_000) as i64];
    matches!(
        call(
            number::PPOLL,
            [watched.as_mut_ptr() as usize, 1, timeout.as_ptr() as usize, 0, 8, 0],
        ),
        Ok(count) if count > 0
    )
}

struct Relay {
    control: usize,
}

struct Answered {
    status: Status,
    start: usize,
    end: usize,
    session: [u8; http::MAX_SESSION_ID_BYTES],
    session_length: usize,
}

impl Answered {
    fn session(&self) -> Option<&[u8]> {
        (self.session_length > 0).then(|| &self.session[..self.session_length])
    }
}

impl Relay {
    fn ask(
        &mut self,
        kind: u32,
        token_hash: &[u8; 32],
        session: Option<&[u8]>,
        body: &[u8],
        answer: &mut [u8],
    ) -> Option<Answered> {
        let mut head = [0u8; relay::REQUEST_HEAD_BYTES];
        let head = relay::request_head(token_hash, session, &mut head);
        if !relay::write_frame(self.control, kind, &[head, body]) {
            return None;
        }
        let (answer_kind, length) = relay::read_frame(self.control, answer)?;
        if answer_kind != relay::kind::ANSWER {
            return None;
        }
        let payload = answer.get(..length)?;
        let (code, session, body) = relay::split_answer(payload)?;
        let mut answered = Answered {
            status: relay::status_of(code),
            start: length - body.len(),
            end: length,
            session: [0; http::MAX_SESSION_ID_BYTES],
            session_length: 0,
        };
        if let Some(session) = session {
            answered.session[..session.len()].copy_from_slice(session);
            answered.session_length = session.len();
        }
        Some(answered)
    }
}

fn answer(
    session: &mut Session<'_>,
    socket: &mut Socket,
    status: Status,
    body: &[u8],
    keep_alive: bool,
    session_id: Option<&[u8]>,
) -> bool {
    let mut head = [0u8; http::MAX_RESPONSE_HEADER_BYTES];
    let length = http::response_head_in_session(&mut head, status, body.len(), keep_alive, session_id);
    session
        .write(socket, head.get(..length).unwrap_or_default())
        .is_ok()
        && (body.is_empty() || session.write(socket, body).is_ok())
}

fn stream_events(
    session: &mut Session<'_>,
    socket: &mut Socket,
    relay: &mut Relay,
    session_id: &[u8],
    buffer: &mut [u8],
) {
    let mut head = [0u8; http::MAX_RESPONSE_HEADER_BYTES];
    let length = http::event_stream_head(&mut head, session_id);
    if session.write(socket, &head[..length]).is_err() {
        return;
    }
    loop {
        if !relay::readable_within(relay.control, STREAM_KEEP_ALIVE_MS) {
            if session.write(socket, http::STREAM_KEEP_ALIVE).is_err() {
                return;
            }
            continue;
        }
        let Some((kind, length)) = relay::read_frame(relay.control, buffer) else {
            return;
        };
        if kind != relay::kind::EVENT {
            continue;
        }
        let line = &buffer[..length];
        if line.contains(&b'\n') {
            continue;
        }
        let written = session.write(socket, b"event: message\ndata: ").is_ok()
            && session.write(socket, line).is_ok()
            && session.write(socket, b"\n\n").is_ok();
        if !written {
            return;
        }
    }
}

fn serve_connection(
    socket: &mut Socket,
    identity: &Identity<'_>,
    tls: &mut records::Buffers,
    input: &mut [u8],
    exchange: &mut Exchange<'_>,
    relay: &mut Relay,
) {
    let Some(randomness) = random::<{ tls_server::RANDOMNESS_BYTES }>() else {
        return;
    };
    socket.deadline_ms = now_ms() + HANDSHAKE_MS;
    let Ok(mut session) = tls_server::accept(socket, tls, identity, &randomness) else {
        return;
    };
    let mut filled = 0;
    socket.deadline_ms = now_ms() + REQUEST_MS;
    loop {
        let parsed = match http::parse(
            input.get(..filled).unwrap_or_default(),
            super::MAX_MESSAGE_BYTES - relay::REQUEST_HEAD_BYTES - relay::FRAME_HEADER_BYTES,
        ) {
            Parsed::Incomplete => None,
            other => Some(other),
        };
        let Some(parsed) = parsed else {
            let Some(free) = input.get_mut(filled..).filter(|free| !free.is_empty()) else {
                let _ = answer(&mut session, socket, Status::PayloadTooLarge, &[], false, None);
                return;
            };
            let waiting_for_first_byte = filled == 0;
            match session.read(socket, free) {
                Ok(0) | Err(_) => return,
                Ok(count) => filled += count,
            }
            if waiting_for_first_byte {
                socket.deadline_ms = socket.deadline_ms.min(now_ms() + REQUEST_MS);
            }
            continue;
        };
        let (request, used) = match parsed {
            Parsed::Complete(request, used) => (request, used),
            Parsed::Refused(status) => {
                let _ = answer(&mut session, socket, status, &[], false, None);
                return;
            }
            Parsed::Incomplete => return,
        };
        let mut error = [0u8; 256];
        let route = http::route(&request);
        let (status, sent) = match route {
            Route::Refused(status, message) => {
                let length = http::error_body(message, &mut error);
                socket.deadline_ms = now_ms() + HANDSHAKE_MS;
                let body = error.get(..length).unwrap_or_default();
                (status, answer(&mut session, socket, status, body, false, None))
            }
            Route::Pair(body) => {
                let now = exchange.system.now();
                let (status, length) = pair(body, now, &mut error);
                socket.deadline_ms = now_ms() + HANDSHAKE_MS;
                let body = error.get(..length).unwrap_or_default();
                (status, answer(&mut session, socket, status, body, false, None))
            }
            Route::Listen(token_hash) => {
                let session_id = request.session_id.unwrap_or_default();
                let answered = relay.ask(
                    relay::kind::LISTEN,
                    &token_hash,
                    Some(session_id),
                    &[],
                    exchange.response,
                );
                match answered.map(|answered| answered.status) {
                    Some(Status::Ok) => {
                        let mut id = [0u8; http::MAX_SESSION_ID_BYTES];
                        id[..session_id.len()].copy_from_slice(session_id);
                        let id = &id[..session_id.len()];
                        stream_events(&mut session, socket, relay, id, exchange.response);
                    }
                    Some(status) => {
                        let length = http::error_body("no such session: initialize again", &mut error);
                        let _ = answer(&mut session, socket, status, &error[..length], false, None);
                    }
                    None => {}
                }
                let _ = session.close(socket);
                return;
            }
            Route::EndSession(token_hash) => {
                let answered = relay.ask(
                    relay::kind::END_SESSION,
                    &token_hash,
                    request.session_id,
                    &[],
                    exchange.response,
                );
                let Some(answered) = answered else {
                    return;
                };
                socket.deadline_ms = now_ms() + IDLE_MS;
                let keep_alive = http::keeps_connection(&route, answered.status, request.keep_alive);
                (
                    answered.status,
                    answer(&mut session, socket, answered.status, &[], keep_alive, None),
                )
            }
            Route::Mcp(token_hash, body) => {
                let stateless = match http::check_stateless_headers(&request) {
                    Ok(stateless) => stateless,
                    Err(message) => {
                        let length = http::header_mismatch_body(message, &mut error);
                        let refusal = error.get(..length).unwrap_or_default();
                        socket.deadline_ms = now_ms() + HANDSHAKE_MS;
                        let _ = answer(&mut session, socket, Status::BadRequest, refusal, false, None);
                        let _ = session.close(socket);
                        return;
                    }
                };
                let Some(answered) = relay.ask(
                    relay::kind::REQUEST,
                    &token_hash,
                    request.session_id,
                    body,
                    exchange.response,
                ) else {
                    return;
                };
                let (start, end) = (answered.start, answered.end);
                let status = match (stateless, answered.status) {
                    (true, Status::Ok) => http::stateless_status(
                        exchange.response.get(start..end).unwrap_or_default(),
                    ),
                    (_, status) => status,
                };
                let keep_alive = http::keeps_connection(&route, status, request.keep_alive);
                socket.deadline_ms = now_ms() + IDLE_MS;
                let sent = match status {
                    Status::Unauthorized => {
                        let length = http::error_body(
                            "this token is no agent's: pair the agent again",
                            &mut error,
                        );
                        let body = error.get(..length).unwrap_or_default();
                        answer(&mut session, socket, status, body, keep_alive, None)
                    }
                    Status::NotFound if request.session_id.is_some() => {
                        let length = http::error_body("no such session: initialize again", &mut error);
                        let body = error.get(..length).unwrap_or_default();
                        answer(&mut session, socket, status, body, keep_alive, None)
                    }
                    _ => {
                        let body = exchange.response.get(start..end).unwrap_or_default();
                        answer(&mut session, socket, status, body, keep_alive, answered.session())
                    }
                };
                (status, sent)
            }
        };
        if !sent || !http::keeps_connection(&route, status, request.keep_alive) {
            let _ = session.close(socket);
            return;
        }
        input.copy_within(used..filled, 0);
        filled -= used;
        socket.deadline_ms = match filled {
            0 => now_ms() + IDLE_MS,
            _ => now_ms() + REQUEST_MS,
        };
    }
}

static mut TLS_BUFFERS: records::Buffers = records::Buffers::new();

#[derive(Clone, Copy)]
struct Connection {
    process: usize,
    control: usize,
    address: [u8; 4],
}

fn socket_pair() -> Result<[usize; 2], Errno> {
    let mut pair = [0i32; 2];
    call(
        number::SOCKETPAIR,
        [AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, pair.as_mut_ptr() as usize, 0, 0],
    )?;
    Ok([pair[0] as usize, pair[1] as usize])
}

fn end_connection(connections: &mut [Option<Connection>], index: usize, sessions: &mut Sessions) {
    let Some(connection) = connections[index].take() else {
        return;
    };
    close(connection.control);
    let mut status = 0i32;
    let _ = call(
        number::WAIT4,
        [connection.process, (&raw mut status) as usize, 0, 0, 0, 0],
    );
    sessions.forget_listener(index);
}

fn accept_connection(
    listener: usize,
    connections: &mut [Option<Connection>],
    identity: &Identity<'_>,
    tls: &mut records::Buffers,
    input: &mut [u8],
    exchange: &mut Exchange<'_>,
) {
    let mut address = [0u8; 16];
    let mut address_length = address.len() as u32;
    let descriptor = match call(
        number::ACCEPT4,
        [
            listener,
            address.as_mut_ptr() as usize,
            (&raw mut address_length) as usize,
            SOCK_CLOEXEC,
            0,
            0,
        ],
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::EINTR | Errno::EAGAIN) => return,
        Err(error) => {
            say(&[b"eggmcpd: accepting an agent failed: ", error.message(), b"\n"]);
            pause(super::RETRY_PAUSE_MS);
            return;
        }
    };
    let peer = [address[4], address[5], address[6], address[7]];
    let open = connections.iter().flatten().count();
    let from_peer = connections
        .iter()
        .flatten()
        .filter(|connection| connection.address == peer)
        .count();
    let free = connections.iter().position(Option::is_none);
    let (Some(free), true) = (free, open < MAX_CONNECTIONS && from_peer < MAX_CONNECTIONS_PER_ADDRESS)
    else {
        close(descriptor);
        return;
    };
    let Ok([parent_end, child_end]) = socket_pair() else {
        close(descriptor);
        return;
    };
    let sigchld = eggos_runtime::signal::SIGCHLD as usize;
    match call(number::CLONE, [sigchld, 0, 0, 0, 0, 0]) {
        Ok(0) => {
            close(listener);
            close(parent_end);
            for connection in connections.iter().flatten() {
                close(connection.control);
            }
            let mut socket = Socket {
                descriptor,
                deadline_ms: 0,
            };
            let mut relay = Relay {
                control: child_end,
            };
            serve_connection(&mut socket, identity, tls, input, exchange, &mut relay);
            close(descriptor);
            syscall::exit(0)
        }
        Ok(process) => {
            close(descriptor);
            close(child_end);
            connections[free] = Some(Connection {
                process,
                control: parent_end,
                address: peer,
            });
        }
        Err(_) => {
            close(descriptor);
            close(parent_end);
            close(child_end);
        }
    }
}

fn answer_connection(
    connections: &mut [Option<Connection>],
    index: usize,
    exchange: &mut Exchange<'_>,
    input: &mut [u8],
) {
    let Some(connection) = connections[index] else {
        return;
    };
    let Some((kind, length)) = relay::read_frame(connection.control, input) else {
        end_connection(connections, index, &mut exchange.sessions);
        return;
    };
    let Some((token_hash, session, body)) = relay::split_request(&input[..length]) else {
        relay::write_answer(connection.control, Status::BadRequest, None, &[]);
        return;
    };
    match kind {
        relay::kind::REQUEST => {
            let outcome = exchange.mcp(&token_hash, session, body);
            let named = outcome.opened.as_ref().map(|id| &id[..]).or(session);
            let answered = exchange.response.get(outcome.start..outcome.end).unwrap_or_default();
            relay::write_answer(connection.control, outcome.status, named, answered);
            let Some(listener) = outcome.listener.and_then(|listener| connections[listener]) else {
                return;
            };
            let notices = exchange.response.get(..outcome.start).unwrap_or_default();
            for line in notices.split(|&byte| byte == b'\n').filter(|line| !line.is_empty()) {
                relay::write_frame(listener.control, relay::kind::EVENT, &[line]);
            }
        }
        relay::kind::LISTEN => {
            let status = match session {
                Some(session) => exchange.listen(&token_hash, session, index),
                None => Status::BadRequest,
            };
            relay::write_answer(connection.control, status, session, &[]);
        }
        relay::kind::END_SESSION => {
            let status = match session {
                Some(session) => exchange.end_session(&token_hash, session),
                None => Status::BadRequest,
            };
            relay::write_answer(connection.control, status, None, &[]);
        }
        _ => {
            relay::write_answer(connection.control, Status::BadRequest, None, &[]);
        }
    }
}

/// Listens for agents on the network and answers them, each connection in a process of its
/// own, until the machine stops.
pub fn run() -> i32 {
    let Some(key) = private_key() else {
        say(&[b"eggmcpd: no key for the network server\n"]);
        return 1;
    };
    let mut certificate = [0u8; MAX_CERTIFICATE_BYTES];
    let certificate_length = load_identity(&key, &mut certificate);
    if certificate_length == 0 {
        say(&[b"eggmcpd: no certificate for the network server\n"]);
        return 1;
    }
    let identity = Identity {
        certificate: certificate.get(..certificate_length).unwrap_or_default(),
        private_key: &key,
        protocol: b"http/1.1",
    };
    let port_number = port();
    let listener = match listen(port_number) {
        Ok(listener) => listener,
        Err(error) => {
            say(&[
                b"eggmcpd: cannot listen for agents: ",
                error.message(),
                b"\n",
            ]);
            return 1;
        }
    };
    ignore_broken_pipes();
    // SAFETY: the program has one thread and takes the buffers once, here, so no other
    // reference to them exists while these live.
    let Some(buffers) = (unsafe { (&raw mut BUFFERS).as_mut() }) else {
        return 1;
    };
    // SAFETY: as above, for the TLS buffers, taken once by the program's one thread.
    let Some(tls) = (unsafe { (&raw mut TLS_BUFFERS).as_mut() }) else {
        return 1;
    };
    let Buffers {
        input,
        response,
        stdout,
        stderr,
        decoded,
        resolved,
        record,
        arguments,
        content,
        words,
        held_path,
        held_arguments,
        command_words,
        child_stack,
    } = buffers;
    let mut exchange = Exchange {
        system: Linux {
            child_stack,
            command_words,
            prompt: None,
            prompts_on_screen: super::screen_has_a_person(),
        },
        scratch: Scratch {
            stdout,
            stderr,
            decoded,
            resolved,
            record,
            arguments,
            content,
            words,
            held_path,
            held_arguments,
        },
        response,
        servers: Servers {
            slots: [const { None }; MAX_SERVERS],
        },
        sessions: Sessions {
            slots: [const { None }; MAX_SESSIONS],
        },
    };
    let mut connections: [Option<Connection>; MAX_CONNECTIONS] = [None; MAX_CONNECTIONS];
    let mut advertiser = advertise::Advertiser::start(port_number, identity.certificate);
    loop {
        let mut watched = [PollDescriptor {
            descriptor: -1,
            events: POLLIN,
            returned_events: 0,
        }; 2 + MAX_CONNECTIONS];
        if connections.iter().flatten().count() < MAX_CONNECTIONS {
            watched[0].descriptor = listener as i32;
        }
        if let Some(advertiser) = &mut advertiser {
            advertiser.tick();
            watched[1].descriptor = advertiser.socket() as i32;
        }
        for (index, connection) in connections.iter().enumerate() {
            if let Some(connection) = connection {
                watched[2 + index].descriptor = connection.control as i32;
            }
        }
        let wait = advertiser.as_ref().map(|advertiser| {
            let left = advertiser.timeout_ms();
            [(left / 1000) as i64, (left % 1000 * 1_000_000) as i64]
        });
        let wait_pointer = wait.as_ref().map_or(0, |wait| wait.as_ptr() as usize);
        match call(
            number::PPOLL,
            [watched.as_mut_ptr() as usize, watched.len(), wait_pointer, 0, 8, 0],
        ) {
            Ok(_) => {}
            Err(_) => continue,
        }
        if watched[1].returned_events & POLLIN != 0
            && let Some(advertiser) = &mut advertiser
        {
            advertiser.readable();
        }
        for index in 0..MAX_CONNECTIONS {
            if watched[2 + index].returned_events & (POLLIN | POLLHUP | POLLERR) != 0 {
                answer_connection(&mut connections, index, &mut exchange, &mut input[..]);
            }
        }
        if watched[0].returned_events & POLLIN != 0 {
            accept_connection(
                listener,
                &mut connections,
                &identity,
                &mut *tls,
                &mut input[..],
                &mut exchange,
            );
        }
    }
}
