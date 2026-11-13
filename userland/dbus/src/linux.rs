//! `session-bus`'s system calls (D575): the listening socket at `/run/user/0/bus`, the loop
//! that waits in `ppoll` for its clients, each client's handshake and messages with the
//! descriptors that come in their control data, and what the bus sends back.

use crate::auth::{Handshake, Progress};
use crate::bus::{Bus, ConnectionId, Credentials, Descriptor};
use crate::message::{self, Message};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

/// The folder of every user's runtime directory.
pub const RUNTIME_PARENT: &CStr = c"/run/user";
/// The runtime directory of root, the account every process runs as.
pub const RUNTIME_DIRECTORY: &CStr = c"/run/user/0";
/// The socket clients connect to: `unix:path=` of `DBUS_SESSION_BUS_ADDRESS`.
pub const SOCKET_PATH: &CStr = c"/run/user/0/bus";
/// The most clients at once.
pub const MAX_CLIENTS: usize = 256;
/// The most descriptors one message may carry.
pub const MAX_DESCRIPTORS: usize = 16;
/// The most bytes waiting to go to one client before the bus drops it.
pub const MAX_QUEUED_BYTES: usize = 64 << 20;

const CONSOLE: usize = 2;
const AT_FDCWD: usize = -100isize as usize;
const AF_UNIX: usize = 1;
const SOCK_STREAM: usize = 1;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2_000_000;
const LISTEN_BACKLOG: usize = 32;
const MSG_DONTWAIT: usize = 0x40;
const MSG_NOSIGNAL: usize = 0x4000;
const MSG_CMSG_CLOEXEC: usize = 0x4000_0000;
const SOL_SOCKET: usize = 1;
const SO_PEERCRED: usize = 17;
const SCM_RIGHTS: u32 = 1;
const CMSG_HEADER: usize = 16;
const CONTROL_WORDS: usize = (CMSG_HEADER + MAX_DESCRIPTORS * 4) / 8;
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 0x10;
const SIGPIPE: usize = eggos_runtime::signal::SIGPIPE as usize;
const SIGNAL_SET_BYTES: usize = eggos_runtime::signal::SET_BYTES;
const RUNTIME_PARENT_MODE: usize = 0o755;
const RUNTIME_DIRECTORY_MODE: usize = 0o700;
const ACCEPTS_PER_WAKE: usize = 8;
const READ_BYTES: usize = 64 * 1024;
const READS_PER_WAKE: usize = 16;

#[repr(C)]
struct SocketAddress {
    family: u16,
    path: [u8; 108],
}

#[repr(C)]
struct IoVector {
    base: usize,
    length: usize,
}

#[repr(C)]
struct MessageHeader {
    name: usize,
    name_length: u32,
    name_padding: u32,
    buffers: usize,
    buffer_count: usize,
    control: usize,
    control_length: usize,
    flags: i32,
    flags_padding: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollEntry {
    descriptor: i32,
    events: i16,
    returned: i16,
}

#[repr(C)]
#[derive(Default)]
struct PeerCredentials {
    pid: u32,
    uid: u32,
    gid: u32,
}

struct Outgoing {
    bytes: Vec<u8>,
    descriptors: Vec<Descriptor>,
    sent: usize,
}

struct Client {
    socket: Descriptor,
    credentials: Credentials,
    handshake: Option<Handshake>,
    connection: Option<ConnectionId>,
    input: Vec<u8>,
    descriptors: VecDeque<Descriptor>,
    output: VecDeque<Outgoing>,
    queued: usize,
    gone: bool,
}

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every call made through here passes only memory this program owns and borrows
    // for the call (paths, socket addresses, message headers with their buffers and control
    // data, poll entries, credentials and random bytes), sized as the call reads or writes it;
    // none maps, unmaps or protects memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn say(parts: &[&[u8]]) {
    for part in parts {
        syscall::write_all(CONSOLE, part);
    }
}

fn close(descriptor: Descriptor) {
    let _ = call(number::CLOSE, [descriptor as usize, 0, 0, 0, 0, 0]);
}

fn address_of(path: &CStr) -> (SocketAddress, usize) {
    let mut address = SocketAddress {
        family: AF_UNIX as u16,
        path: [0; 108],
    };
    let bytes = path.to_bytes();
    let length = bytes.len().min(address.path.len() - 1);
    address.path[..length].copy_from_slice(&bytes[..length]);
    (address, 2 + length + 1)
}

fn new_socket(flags: usize) -> Result<Descriptor, Errno> {
    call(number::SOCKET, [AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | flags, 0, 0, 0, 0])
        .map(|descriptor| descriptor as Descriptor)
}

fn someone_listens() -> bool {
    let Ok(socket) = new_socket(0) else {
        return false;
    };
    let (address, length) = address_of(SOCKET_PATH);
    let connected = call(number::CONNECT, [socket as usize, &raw const address as usize, length, 0, 0, 0]).is_ok();
    close(socket);
    connected
}

fn listen() -> Result<Descriptor, Errno> {
    for (folder, mode) in [(RUNTIME_PARENT, RUNTIME_PARENT_MODE), (RUNTIME_DIRECTORY, RUNTIME_DIRECTORY_MODE)] {
        match call(number::MKDIRAT, [AT_FDCWD, folder.as_ptr() as usize, mode, 0, 0, 0]) {
            Ok(_) | Err(Errno::EEXIST) => {}
            Err(error) => return Err(error),
        }
    }
    let _ = call(number::UNLINKAT, [AT_FDCWD, SOCKET_PATH.as_ptr() as usize, 0, 0, 0, 0]);
    let socket = new_socket(SOCK_NONBLOCK)?;
    let (address, length) = address_of(SOCKET_PATH);
    let bound = call(number::BIND, [socket as usize, &raw const address as usize, length, 0, 0, 0])
        .and_then(|_| call(number::LISTEN, [socket as usize, LISTEN_BACKLOG, 0, 0, 0, 0]));
    if let Err(error) = bound {
        close(socket);
        return Err(error);
    }
    Ok(socket)
}

fn ignore_broken_pipes() {
    let action = eggos_runtime::signal::action(eggos_runtime::signal::IGNORE);
    let _ = call(number::RT_SIGACTION, [SIGPIPE, action.as_ptr() as usize, 0, SIGNAL_SET_BYTES, 0, 0]);
}

fn bus_id() -> [u8; 32] {
    let mut random = [0u8; 16];
    let _ = call(number::GETRANDOM, [random.as_mut_ptr() as usize, random.len(), 0, 0, 0, 0]);
    let mut id = [0u8; 32];
    for (index, byte) in random.iter().enumerate() {
        id[2 * index] = b"0123456789abcdef"[usize::from(byte >> 4)];
        id[2 * index + 1] = b"0123456789abcdef"[usize::from(byte & 15)];
    }
    id
}

fn peer_credentials(socket: Descriptor) -> Credentials {
    let mut peer = PeerCredentials::default();
    let mut length = size_of::<PeerCredentials>() as u32;
    let _ = call(
        number::GETSOCKOPT,
        [
            socket as usize,
            SOL_SOCKET,
            SO_PEERCRED,
            &raw mut peer as usize,
            &raw mut length as usize,
            0,
        ],
    );
    Credentials {
        uid: peer.uid,
        pid: peer.pid,
    }
}

fn receive(socket: Descriptor, bytes: &mut [u8], descriptors: &mut VecDeque<Descriptor>) -> Result<usize, Errno> {
    let mut vector = IoVector {
        base: bytes.as_mut_ptr() as usize,
        length: bytes.len(),
    };
    let mut control = [0u64; CONTROL_WORDS];
    let mut header = MessageHeader {
        name: 0,
        name_length: 0,
        name_padding: 0,
        buffers: &raw mut vector as usize,
        buffer_count: 1,
        control: control.as_mut_ptr() as usize,
        control_length: CONTROL_WORDS * 8,
        flags: 0,
        flags_padding: 0,
    };
    let received = call(
        number::RECVMSG,
        [socket as usize, &raw mut header as usize, MSG_DONTWAIT | MSG_CMSG_CLOEXEC, 0, 0, 0],
    )?;
    let mut control_bytes = [0u8; CONTROL_WORDS * 8];
    for (chunk, word) in control_bytes.as_chunks_mut::<8>().0.iter_mut().zip(control) {
        *chunk = word.to_ne_bytes();
    }
    let control = &control_bytes[..header.control_length.min(CONTROL_WORDS * 8)];
    let mut at = 0;
    while at + CMSG_HEADER <= control.len() {
        let length = u64::from_ne_bytes(control[at..at + 8].try_into().unwrap_or([0; 8])) as usize;
        let level = u32::from_ne_bytes(control[at + 8..at + 12].try_into().unwrap_or([0; 4]));
        let kind = u32::from_ne_bytes(control[at + 12..at + 16].try_into().unwrap_or([0; 4]));
        if length < CMSG_HEADER || at + length > control.len() {
            break;
        }
        if level as usize == SOL_SOCKET && kind == SCM_RIGHTS {
            for chunk in control[at + CMSG_HEADER..at + length].as_chunks::<4>().0 {
                descriptors.push_back(i32::from_ne_bytes(*chunk));
            }
        }
        at += length.div_ceil(8) * 8;
    }
    Ok(received)
}

fn send(socket: Descriptor, bytes: &[u8], descriptors: &[Descriptor]) -> Result<usize, Errno> {
    let mut vector = IoVector {
        base: bytes.as_ptr() as usize,
        length: bytes.len(),
    };
    let count = descriptors.len().min(MAX_DESCRIPTORS);
    let mut control = [0u8; CONTROL_WORDS * 8];
    let control_length = match count {
        0 => 0,
        count => {
            let length = CMSG_HEADER + count * 4;
            control[..8].copy_from_slice(&(length as u64).to_ne_bytes());
            control[8..12].copy_from_slice(&(SOL_SOCKET as u32).to_ne_bytes());
            control[12..16].copy_from_slice(&SCM_RIGHTS.to_ne_bytes());
            for (index, &descriptor) in descriptors[..count].iter().enumerate() {
                let at = CMSG_HEADER + index * 4;
                control[at..at + 4].copy_from_slice(&descriptor.to_ne_bytes());
            }
            length.div_ceil(8) * 8
        }
    };
    let mut aligned = [0u64; CONTROL_WORDS];
    for (word, chunk) in aligned.iter_mut().zip(control.as_chunks::<8>().0) {
        *word = u64::from_ne_bytes(*chunk);
    }
    let mut header = MessageHeader {
        name: 0,
        name_length: 0,
        name_padding: 0,
        buffers: &raw mut vector as usize,
        buffer_count: 1,
        control: match control_length {
            0 => 0,
            _ => aligned.as_mut_ptr() as usize,
        },
        control_length,
        flags: 0,
        flags_padding: 0,
    };
    call(
        number::SENDMSG,
        [socket as usize, &raw mut header as usize, MSG_DONTWAIT | MSG_NOSIGNAL, 0, 0, 0],
    )
}

impl Client {
    fn new(socket: Descriptor, id: [u8; 32]) -> Self {
        let credentials = peer_credentials(socket);
        Self {
            socket,
            credentials,
            handshake: Some(Handshake::new(credentials.uid, id)),
            connection: None,
            input: Vec::new(),
            descriptors: VecDeque::new(),
            output: VecDeque::new(),
            queued: 0,
            gone: false,
        }
    }

    fn queue(&mut self, bytes: Vec<u8>, descriptors: Vec<Descriptor>) {
        self.queued += bytes.len();
        self.output.push_back(Outgoing {
            bytes,
            descriptors,
            sent: 0,
        });
        if self.queued > MAX_QUEUED_BYTES {
            self.gone = true;
        }
    }

    fn flush(&mut self) {
        while let Some(front) = self.output.front_mut() {
            let descriptors: &[Descriptor] = match front.sent {
                0 => &front.descriptors,
                _ => &[],
            };
            match send(self.socket, &front.bytes[front.sent..], descriptors) {
                Ok(count) => {
                    front.sent += count;
                    self.queued -= count;
                    if front.sent < front.bytes.len() {
                        return;
                    }
                    if let Some(done) = self.output.pop_front() {
                        done.descriptors.into_iter().for_each(close);
                    }
                }
                Err(Errno::EAGAIN) => return,
                Err(_) => {
                    self.gone = true;
                    return;
                }
            }
        }
    }

    fn read(&mut self, bus: &mut Bus, buffer: &mut [u8]) {
        for _ in 0..READS_PER_WAKE {
            let count = match receive(self.socket, buffer, &mut self.descriptors) {
                Ok(0) => {
                    self.gone = true;
                    return;
                }
                Ok(count) => count,
                Err(Errno::EAGAIN | Errno::EINTR) => return,
                Err(_) => {
                    self.gone = true;
                    return;
                }
            };
            let mut taken = &buffer[..count];
            if let Some(handshake) = &mut self.handshake {
                let mut replies = Vec::new();
                let progress = handshake.feed(taken, &mut replies);
                let unix_fds = handshake.unix_fds();
                if !replies.is_empty() {
                    self.queue(replies, Vec::new());
                }
                match progress {
                    Progress::Going(_) => continue,
                    Progress::Failed => {
                        self.gone = true;
                        return;
                    }
                    Progress::Begun(used) => {
                        self.handshake = None;
                        self.descriptors.drain(..).for_each(close);
                        self.connection = Some(bus.connect(self.credentials, unix_fds));
                        taken = &taken[used..];
                    }
                }
            }
            self.input.extend_from_slice(taken);
            self.take_messages(bus);
            if self.gone {
                return;
            }
        }
    }

    fn take_messages(&mut self, bus: &mut Bus) {
        let Some(connection) = self.connection else {
            return;
        };
        loop {
            let length = match message::length(&self.input) {
                Ok(Some(length)) if length <= self.input.len() => length,
                Ok(_) => return,
                Err(_) => {
                    self.gone = true;
                    return;
                }
            };
            let wanted = Message::parse(&self.input[..length]).map_or(0, |message| message.fields.unix_fds as usize);
            let count = wanted.min(self.descriptors.len()).min(MAX_DESCRIPTORS);
            let descriptors: Vec<Descriptor> = self.descriptors.drain(..count).collect();
            bus.receive(connection, &self.input[..length], descriptors);
            self.input.drain(..length);
        }
    }

    fn close(self) {
        close(self.socket);
        self.descriptors.into_iter().for_each(close);
        for outgoing in self.output {
            outgoing.descriptors.into_iter().for_each(close);
        }
    }
}

/// `session-bus`'s main function: serves the session bus until it fails.
pub fn run(_startup: &Startup) -> i32 {
    ignore_broken_pipes();
    if someone_listens() {
        say(&[b"session-bus: a bus already runs on /run/user/0/bus\n"]);
        return 1;
    }
    let listener = match listen() {
        Ok(listener) => listener,
        Err(error) => {
            say(&[b"session-bus: cannot listen on /run/user/0/bus: ", error.message(), b"\n"]);
            return 1;
        }
    };
    serve(listener)
}

fn serve(listener: Descriptor) -> ! {
    let mut bus = Bus::new(bus_id());
    let mut clients: Vec<Client> = Vec::new();
    let mut entries: Vec<PollEntry> = Vec::new();
    let mut buffer = alloc::vec![0u8; READ_BYTES];
    loop {
        entries.clear();
        entries.push(PollEntry {
            descriptor: match clients.len() < MAX_CLIENTS {
                true => listener,
                false => -1,
            },
            events: POLLIN,
            returned: 0,
        });
        for client in &clients {
            let mut events = POLLIN;
            if !client.output.is_empty() {
                events |= POLLOUT;
            }
            entries.push(PollEntry {
                descriptor: client.socket,
                events,
                returned: 0,
            });
        }
        let _ = call(number::PPOLL, [entries.as_mut_ptr() as usize, entries.len(), 0, 0, 8, 0]);
        if entries[0].returned & POLLIN != 0 {
            for _ in 0..ACCEPTS_PER_WAKE {
                let Ok(socket) = call(number::ACCEPT4, [listener as usize, 0, 0, SOCK_NONBLOCK | SOCK_CLOEXEC, 0, 0]) else {
                    break;
                };
                clients.push(Client::new(socket as Descriptor, bus.id()));
            }
        }
        for (index, entry) in entries.iter().enumerate().skip(1) {
            let Some(client) = clients.get_mut(index - 1) else {
                continue;
            };
            if entry.returned & (POLLIN | POLLHUP | POLLERR) != 0 {
                client.read(&mut bus, &mut buffer);
            }
        }
        for closing in bus.take_closing() {
            if let Some(client) = clients.iter_mut().find(|client| client.connection == Some(closing)) {
                client.gone = true;
            }
        }
        loop {
            let mut ended = Vec::new();
            for (index, client) in clients.iter().enumerate() {
                if client.gone {
                    ended.push(index);
                }
            }
            if ended.is_empty() {
                break;
            }
            for index in ended.into_iter().rev() {
                let client = clients.swap_remove(index);
                if let Some(connection) = client.connection {
                    bus.disconnect(connection);
                }
                client.close();
            }
            deliver(&mut bus, &mut clients);
        }
        deliver(&mut bus, &mut clients);
        bus.take_unused_descriptors().into_iter().for_each(close);
        for client in &mut clients {
            if !client.output.is_empty() {
                client.flush();
            }
        }
    }
}

fn deliver(bus: &mut Bus, clients: &mut [Client]) {
    for delivery in bus.take_deliveries() {
        match clients.iter_mut().find(|client| client.connection == Some(delivery.to)) {
            Some(client) if !client.gone => client.queue(delivery.bytes, delivery.descriptors),
            _ => delivery.descriptors.into_iter().for_each(close),
        }
    }
}
