//! The message bus: connections and their unique names, well-known names with their queues of
//! owners, the routing of calls, replies and signals, and the bus's own interface,
//! `org.freedesktop.DBus`. It reads messages and writes the ones to send; the program that
//! runs it moves the bytes.

use crate::message::{self, Fields, Malformed, Message, flag, kind};
use crate::rules::Rule;
use crate::wire::{Endian, Writer};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// A connection's number on the bus.
pub type ConnectionId = usize;
/// A descriptor passed with a message.
pub type Descriptor = i32;

/// The bus's own name.
pub const BUS_NAME: &[u8] = b"org.freedesktop.DBus";
/// The bus's object path.
pub const BUS_PATH: &[u8] = b"/org/freedesktop/DBus";
/// The most match rules one connection may hold.
pub const MAX_RULES_PER_CONNECTION: usize = 512;
/// The most well-known names one connection may own or wait for.
pub const MAX_NAMES_PER_CONNECTION: usize = 512;
/// The most calls one connection may wait on replies to.
pub const MAX_PENDING_PER_CONNECTION: usize = 1024;

/// `RequestName`'s flags.
pub mod request {
    pub const ALLOW_REPLACEMENT: u32 = 1;
    pub const REPLACE_EXISTING: u32 = 2;
    pub const DO_NOT_QUEUE: u32 = 4;
    pub const PRIMARY_OWNER: u32 = 1;
    pub const IN_QUEUE: u32 = 2;
    pub const EXISTS: u32 = 3;
    pub const ALREADY_OWNER: u32 = 4;
}

/// `ReleaseName`'s replies.
pub mod release {
    pub const RELEASED: u32 = 1;
    pub const NON_EXISTENT: u32 = 2;
    pub const NOT_OWNER: u32 = 3;
}

mod error {
    pub const FAILED: &[u8] = b"org.freedesktop.DBus.Error.Failed";
    pub const SERVICE_UNKNOWN: &[u8] = b"org.freedesktop.DBus.Error.ServiceUnknown";
    pub const NAME_HAS_NO_OWNER: &[u8] = b"org.freedesktop.DBus.Error.NameHasNoOwner";
    pub const UNKNOWN_METHOD: &[u8] = b"org.freedesktop.DBus.Error.UnknownMethod";
    pub const INVALID_ARGS: &[u8] = b"org.freedesktop.DBus.Error.InvalidArgs";
    pub const MATCH_RULE_INVALID: &[u8] = b"org.freedesktop.DBus.Error.MatchRuleInvalid";
    pub const MATCH_RULE_NOT_FOUND: &[u8] = b"org.freedesktop.DBus.Error.MatchRuleNotFound";
    pub const LIMITS_EXCEEDED: &[u8] = b"org.freedesktop.DBus.Error.LimitsExceeded";
    pub const NO_REPLY: &[u8] = b"org.freedesktop.DBus.Error.NoReply";
    pub const ACCESS_DENIED: &[u8] = b"org.freedesktop.DBus.Error.AccessDenied";
}

const INTROSPECTION: &[u8] = b"<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n<interface name=\"org.freedesktop.DBus\">\n<method name=\"Hello\"><arg direction=\"out\" type=\"s\"/></method>\n<method name=\"RequestName\"><arg direction=\"in\" type=\"s\"/><arg direction=\"in\" type=\"u\"/><arg direction=\"out\" type=\"u\"/></method>\n<method name=\"ReleaseName\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"u\"/></method>\n<method name=\"GetNameOwner\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"s\"/></method>\n<method name=\"NameHasOwner\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"b\"/></method>\n<method name=\"ListNames\"><arg direction=\"out\" type=\"as\"/></method>\n<method name=\"ListActivatableNames\"><arg direction=\"out\" type=\"as\"/></method>\n<method name=\"ListQueuedOwners\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"as\"/></method>\n<method name=\"AddMatch\"><arg direction=\"in\" type=\"s\"/></method>\n<method name=\"RemoveMatch\"><arg direction=\"in\" type=\"s\"/></method>\n<method name=\"GetId\"><arg direction=\"out\" type=\"s\"/></method>\n<method name=\"StartServiceByName\"><arg direction=\"in\" type=\"s\"/><arg direction=\"in\" type=\"u\"/><arg direction=\"out\" type=\"u\"/></method>\n<method name=\"UpdateActivationEnvironment\"><arg direction=\"in\" type=\"a{ss}\"/></method>\n<method name=\"GetConnectionUnixUser\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"u\"/></method>\n<method name=\"GetConnectionUnixProcessID\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"u\"/></method>\n<method name=\"GetConnectionCredentials\"><arg direction=\"in\" type=\"s\"/><arg direction=\"out\" type=\"a{sv}\"/></method>\n<signal name=\"NameOwnerChanged\"><arg type=\"s\"/><arg type=\"s\"/><arg type=\"s\"/></signal>\n<signal name=\"NameLost\"><arg type=\"s\"/></signal>\n<signal name=\"NameAcquired\"><arg type=\"s\"/></signal>\n</interface>\n<interface name=\"org.freedesktop.DBus.Peer\">\n<method name=\"Ping\"/>\n<method name=\"GetMachineId\"><arg direction=\"out\" type=\"s\"/></method>\n</interface>\n<interface name=\"org.freedesktop.DBus.Introspectable\">\n<method name=\"Introspect\"><arg direction=\"out\" type=\"s\"/></method>\n</interface>\n</node>\n";

/// Who is at the other end of a connection, as the kernel reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credentials {
    pub uid: u32,
    pub pid: u32,
}

/// Bytes to send to a connection, with the descriptors that go with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub to: ConnectionId,
    pub bytes: Vec<u8>,
    pub descriptors: Vec<Descriptor>,
}

#[derive(Debug)]
struct Connection {
    unique: Vec<u8>,
    greeted: bool,
    credentials: Credentials,
    unix_fds: bool,
    rules: Vec<Rule>,
    names: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pending {
    replier: ConnectionId,
    caller: ConnectionId,
    serial: u32,
}

/// The bus. See the module documentation.
#[derive(Debug)]
pub struct Bus {
    id: [u8; 32],
    connections: Vec<Option<Connection>>,
    names: BTreeMap<Vec<u8>, Vec<(ConnectionId, u32)>>,
    pending: Vec<Pending>,
    next_unique: u64,
    serial: u32,
    deliveries: Vec<Delivery>,
    closing: Vec<ConnectionId>,
    unused_descriptors: Vec<Descriptor>,
}

/// Whether `name` is a valid well-known bus name.
pub fn valid_well_known_name(name: &[u8]) -> bool {
    if name.is_empty() || name.len() > 255 || name[0] == b':' {
        return false;
    }
    let mut elements = 0;
    for element in name.split(|&byte| byte == b'.') {
        let valid = !element.is_empty()
            && !element[0].is_ascii_digit()
            && element.iter().all(|&byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !valid {
            return false;
        }
        elements += 1;
    }
    elements >= 2
}

fn decimal(writer: &mut Vec<u8>, number: u64) {
    let start = writer.len();
    let mut rest = number;
    loop {
        writer.push(b'0' + (rest % 10) as u8);
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    writer[start..].reverse();
}

impl Bus {
    /// A bus whose id (`GetId`, and the GUID the handshake sends) is `id`, 32 hex digits.
    pub fn new(id: [u8; 32]) -> Self {
        Self {
            id,
            connections: Vec::new(),
            names: BTreeMap::new(),
            pending: Vec::new(),
            next_unique: 1,
            serial: 0,
            deliveries: Vec::new(),
            closing: Vec::new(),
            unused_descriptors: Vec::new(),
        }
    }

    /// The bus's id.
    pub fn id(&self) -> [u8; 32] {
        self.id
    }

    /// A new connection that finished its handshake, from `credentials`, passing descriptors
    /// when `unix_fds`.
    pub fn connect(&mut self, credentials: Credentials, unix_fds: bool) -> ConnectionId {
        let mut unique = Vec::from(&b":1."[..]);
        decimal(&mut unique, self.next_unique);
        self.next_unique += 1;
        let connection = Connection {
            unique,
            greeted: false,
            credentials,
            unix_fds,
            rules: Vec::new(),
            names: 0,
        };
        match self.connections.iter().position(Option::is_none) {
            Some(free) => {
                self.connections[free] = Some(connection);
                free
            }
            None => {
                self.connections.push(Some(connection));
                self.connections.len() - 1
            }
        }
    }

    /// The messages to send, taken.
    pub fn take_deliveries(&mut self) -> Vec<Delivery> {
        core::mem::take(&mut self.deliveries)
    }

    /// The connections the bus dropped for breaking the protocol, taken: close them.
    pub fn take_closing(&mut self) -> Vec<ConnectionId> {
        core::mem::take(&mut self.closing)
    }

    /// Descriptors that came with messages nobody received, taken: close them.
    pub fn take_unused_descriptors(&mut self) -> Vec<Descriptor> {
        core::mem::take(&mut self.unused_descriptors)
    }

    fn connection(&self, id: ConnectionId) -> Option<&Connection> {
        self.connections.get(id)?.as_ref()
    }

    fn unique(&self, id: ConnectionId) -> &[u8] {
        self.connection(id).map_or(&b""[..], |connection| &connection.unique)
    }

    fn by_unique(&self, name: &[u8]) -> Option<ConnectionId> {
        self.connections.iter().position(|connection| {
            connection
                .as_ref()
                .is_some_and(|connection| connection.greeted && connection.unique == name)
        })
    }

    fn owner(&self, name: &[u8]) -> Option<ConnectionId> {
        match name.first() {
            Some(b':') => self.by_unique(name),
            _ => self.names.get(name).and_then(|queue| queue.first()).map(|&(owner, _)| owner),
        }
    }

    fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    /// Ends connection `id`: its names pass to the next in their queues, callers waiting on it
    /// get `NoReply`, and everyone watching sees it go.
    pub fn disconnect(&mut self, id: ConnectionId) {
        let Some(connection) = self.connections.get_mut(id).and_then(Option::take) else {
            return;
        };
        let held: Vec<Vec<u8>> = self
            .names
            .iter()
            .filter(|(_, queue)| queue.iter().any(|&(owner, _)| owner == id))
            .map(|(name, _)| name.clone())
            .collect();
        for name in held {
            self.release(&name, id, &connection.unique, false);
        }
        let waiting: Vec<Pending> = self.pending.iter().copied().filter(|pending| pending.replier == id).collect();
        self.pending.retain(|pending| pending.replier != id && pending.caller != id);
        for pending in waiting {
            if self.connection(pending.caller).is_some() {
                self.error_to(
                    pending.caller,
                    pending.serial,
                    error::NO_REPLY,
                    b"The peer went away before replying",
                );
            }
        }
        if connection.greeted {
            self.name_owner_changed(&connection.unique, &connection.unique, b"");
        }
    }

    /// Takes the message `bytes` from connection `from`, with the descriptors that came with
    /// it.
    pub fn receive(&mut self, from: ConnectionId, bytes: &[u8], descriptors: Vec<Descriptor>) {
        let Some(connection) = self.connection(from) else {
            self.unused_descriptors.extend(descriptors);
            return;
        };
        let greeted = connection.greeted;
        let parsed = Message::parse(bytes);
        let message = match parsed {
            Ok(message) if message.fields.unix_fds as usize == descriptors.len() => message,
            Ok(_) | Err(Malformed::Kind) if greeted => {
                self.unused_descriptors.extend(descriptors);
                return;
            }
            _ => {
                self.unused_descriptors.extend(descriptors);
                self.closing.push(from);
                return;
            }
        };
        if !greeted {
            let is_hello = message.kind == kind::METHOD_CALL
                && message.fields.destination == Some(BUS_NAME)
                && message.fields.member == Some(b"Hello");
            if !is_hello {
                self.unused_descriptors.extend(descriptors);
                self.closing.push(from);
                return;
            }
        }
        let to_bus = match message.fields.destination {
            Some(BUS_NAME) => true,
            None => message.kind == kind::METHOD_CALL,
            Some(_) => false,
        };
        if to_bus {
            self.unused_descriptors.extend(descriptors);
            if message.kind == kind::METHOD_CALL {
                self.call_bus(from, &message);
            }
            return;
        }
        let sender = self.unique(from).to_vec();
        let routed = message.with_sender(&sender);
        match message.fields.destination {
            Some(destination) => self.unicast(from, &message, destination, routed, descriptors),
            None => {
                self.unused_descriptors.extend(descriptors);
                if message.kind == kind::SIGNAL && message.fields.unix_fds == 0 {
                    self.broadcast(from, &routed);
                }
            }
        }
    }

    fn unicast(
        &mut self,
        from: ConnectionId,
        message: &Message<'_>,
        destination: &[u8],
        routed: Vec<u8>,
        descriptors: Vec<Descriptor>,
    ) {
        let Some(to) = self.owner(destination) else {
            self.unused_descriptors.extend(descriptors);
            if message.expects_reply() {
                let mut text = Vec::from(&b"The name "[..]);
                text.extend_from_slice(destination);
                text.extend_from_slice(b" was not provided by any .service files");
                self.error_to(from, message.serial, error::SERVICE_UNKNOWN, &text);
            }
            return;
        };
        let is_reply = matches!(message.kind, kind::METHOD_RETURN | kind::ERROR);
        if is_reply {
            let reply_serial = message.fields.reply_serial.unwrap_or(0);
            let expected = Pending {
                replier: from,
                caller: to,
                serial: reply_serial,
            };
            let Some(at) = self.pending.iter().position(|&pending| pending == expected) else {
                self.unused_descriptors.extend(descriptors);
                return;
            };
            self.pending.swap_remove(at);
        }
        let takes_descriptors = self.connection(to).is_some_and(|connection| connection.unix_fds);
        if !descriptors.is_empty() && !takes_descriptors {
            self.unused_descriptors.extend(descriptors);
            if message.expects_reply() {
                self.error_to(
                    from,
                    message.serial,
                    error::FAILED,
                    b"The recipient does not accept file descriptors",
                );
            }
            return;
        }
        if message.expects_reply() {
            let waiting = self.pending.iter().filter(|pending| pending.caller == from).count();
            if waiting >= MAX_PENDING_PER_CONNECTION {
                self.unused_descriptors.extend(descriptors);
                self.error_to(from, message.serial, error::LIMITS_EXCEEDED, b"Too many calls waiting for replies");
                return;
            }
            self.pending.push(Pending {
                replier: to,
                caller: from,
                serial: message.serial,
            });
        }
        self.deliveries.push(Delivery {
            to,
            bytes: routed,
            descriptors,
        });
    }

    fn broadcast(&mut self, from: ConnectionId, routed: &[u8]) {
        let Ok(message) = Message::parse(routed) else {
            return;
        };
        let mut receivers = Vec::new();
        for (id, connection) in self.connections.iter().enumerate() {
            let Some(connection) = connection else {
                continue;
            };
            if !connection.greeted {
                continue;
            }
            let is_sender = |name: &[u8]| match name.first() {
                Some(b':') => self.by_unique(name) == Some(from),
                _ if name == BUS_NAME => from == usize::MAX,
                _ => self.owner(name) == Some(from),
            };
            if connection.rules.iter().any(|rule| rule.matches(&message, is_sender)) {
                receivers.push(id);
            }
        }
        for to in receivers {
            self.deliveries.push(Delivery {
                to,
                bytes: routed.to_vec(),
                descriptors: Vec::new(),
            });
        }
    }

    fn send_from_bus(&mut self, kind: u8, fields: Fields<'_>, body: &Writer) -> Vec<u8> {
        let serial = self.next_serial();
        let fields = Fields {
            sender: Some(BUS_NAME),
            ..fields
        };
        message::build(Endian::Little, kind, flag::NO_REPLY_EXPECTED, serial, &fields, body.bytes())
    }

    fn reply_to(&mut self, to: ConnectionId, reply_serial: u32, signature: &[u8], body: &Writer) {
        let unique = self.unique(to).to_vec();
        let fields = Fields {
            reply_serial: Some(reply_serial),
            destination: Some(&unique),
            signature,
            ..Fields::default()
        };
        let bytes = self.send_from_bus(kind::METHOD_RETURN, fields, body);
        self.deliveries.push(Delivery {
            to,
            bytes,
            descriptors: Vec::new(),
        });
    }

    fn error_to(&mut self, to: ConnectionId, reply_serial: u32, name: &[u8], text: &[u8]) {
        let unique = self.unique(to).to_vec();
        let mut body = Writer::new(Endian::Little);
        body.string(text);
        let fields = Fields {
            reply_serial: Some(reply_serial),
            destination: Some(&unique),
            error_name: Some(name),
            signature: b"s",
            ..Fields::default()
        };
        let bytes = self.send_from_bus(kind::ERROR, fields, &body);
        self.deliveries.push(Delivery {
            to,
            bytes,
            descriptors: Vec::new(),
        });
    }

    fn signal_to(&mut self, to: ConnectionId, member: &[u8], name: &[u8]) {
        let unique = self.unique(to).to_vec();
        let mut body = Writer::new(Endian::Little);
        body.string(name);
        let fields = Fields {
            path: Some(BUS_PATH),
            interface: Some(BUS_NAME),
            member: Some(member),
            destination: Some(&unique),
            signature: b"s",
            ..Fields::default()
        };
        let bytes = self.send_from_bus(kind::SIGNAL, fields, &body);
        self.deliveries.push(Delivery {
            to,
            bytes,
            descriptors: Vec::new(),
        });
    }

    fn name_owner_changed(&mut self, name: &[u8], old: &[u8], new: &[u8]) {
        let mut body = Writer::new(Endian::Little);
        body.string(name);
        body.string(old);
        body.string(new);
        let fields = Fields {
            path: Some(BUS_PATH),
            interface: Some(BUS_NAME),
            member: Some(b"NameOwnerChanged"),
            signature: b"sss",
            ..Fields::default()
        };
        let bytes = self.send_from_bus(kind::SIGNAL, fields, &body);
        self.broadcast(usize::MAX, &bytes);
    }

    fn call_bus(&mut self, from: ConnectionId, message: &Message<'_>) {
        let interface = message.fields.interface.unwrap_or(BUS_NAME);
        let member = message.fields.member.unwrap_or(b"");
        let serial = message.serial;
        let signature = message.fields.signature;
        let mut reader = message.body_reader();
        let mut out = Writer::new(Endian::Little);
        let mut first_string = || reader.string().ok();
        let greeted = self.connection(from).is_some_and(|connection| connection.greeted);
        match (interface, member, signature) {
            (BUS_NAME, b"Hello", b"") => {
                if greeted {
                    self.error_to(from, serial, error::FAILED, b"Already handled an Hello message");
                    return;
                }
                if let Some(Some(connection)) = self.connections.get_mut(from) {
                    connection.greeted = true;
                }
                let unique = self.unique(from).to_vec();
                out.string(&unique);
                self.reply_to(from, serial, b"s", &out);
                self.signal_to(from, b"NameAcquired", &unique);
                self.name_owner_changed(&unique, b"", &unique);
            }
            (BUS_NAME, b"RequestName", b"su") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                let name = name.to_vec();
                let flags = reader.u32().unwrap_or(0);
                if !valid_well_known_name(&name) || name == BUS_NAME {
                    return self.invalid(from, serial);
                }
                match self.request_name(from, &name, flags) {
                    Some((answer, previous)) => {
                        out.u32(answer);
                        self.reply_to(from, serial, b"u", &out);
                        if answer == request::PRIMARY_OWNER {
                            self.after_new_owner(&name, from, &previous);
                        }
                    }
                    None => self.error_to(from, serial, error::LIMITS_EXCEEDED, b"Too many names"),
                }
            }
            (BUS_NAME, b"ReleaseName", b"s") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                let name = name.to_vec();
                if !valid_well_known_name(&name) {
                    return self.invalid(from, serial);
                }
                let unique = self.unique(from).to_vec();
                let answer = self.release(&name, from, &unique, true);
                out.u32(answer);
                self.reply_to(from, serial, b"u", &out);
            }
            (BUS_NAME, b"GetNameOwner", b"s") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                let name = name.to_vec();
                let owner = match name.as_slice() {
                    BUS_NAME => Some(BUS_NAME.to_vec()),
                    _ => self.owner(&name).map(|owner| self.unique(owner).to_vec()),
                };
                match owner {
                    Some(owner) => {
                        out.string(&owner);
                        self.reply_to(from, serial, b"s", &out);
                    }
                    None => self.no_owner(from, serial, &name),
                }
            }
            (BUS_NAME, b"NameHasOwner", b"s") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                out.boolean(name == BUS_NAME || self.owner(name).is_some());
                self.reply_to(from, serial, b"b", &out);
            }
            (BUS_NAME, b"ListNames", b"") => {
                let mut names: Vec<Vec<u8>> = Vec::from([BUS_NAME.to_vec()]);
                names.extend(self.names.keys().cloned());
                names.extend(
                    self.connections
                        .iter()
                        .flatten()
                        .filter(|connection| connection.greeted)
                        .map(|connection| connection.unique.clone()),
                );
                string_array(&mut out, &names);
                self.reply_to(from, serial, b"as", &out);
            }
            (BUS_NAME, b"ListActivatableNames", b"") => {
                string_array(&mut out, &[BUS_NAME.to_vec()]);
                self.reply_to(from, serial, b"as", &out);
            }
            (BUS_NAME, b"ListQueuedOwners", b"s") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                let name = name.to_vec();
                let owners: Option<Vec<Vec<u8>>> = match name.first() {
                    Some(b':') => self.by_unique(&name).map(|_| Vec::from([name.clone()])),
                    _ => self.names.get(&name).map(|queue| {
                        queue.iter().map(|&(owner, _)| self.unique(owner).to_vec()).collect()
                    }),
                };
                match owners {
                    Some(owners) => {
                        string_array(&mut out, &owners);
                        self.reply_to(from, serial, b"as", &out);
                    }
                    None => self.no_owner(from, serial, &name),
                }
            }
            (BUS_NAME, b"AddMatch", b"s") => {
                let Some(text) = first_string() else {
                    return self.invalid(from, serial);
                };
                let Some(rule) = Rule::parse(text) else {
                    return self.error_to(from, serial, error::MATCH_RULE_INVALID, b"Invalid match rule");
                };
                let Some(Some(connection)) = self.connections.get_mut(from) else {
                    return;
                };
                if connection.rules.len() >= MAX_RULES_PER_CONNECTION {
                    return self.error_to(from, serial, error::LIMITS_EXCEEDED, b"Too many match rules");
                }
                connection.rules.push(rule);
                self.reply_to(from, serial, b"", &out);
            }
            (BUS_NAME, b"RemoveMatch", b"s") => {
                let Some(text) = first_string() else {
                    return self.invalid(from, serial);
                };
                let Some(rule) = Rule::parse(text) else {
                    return self.error_to(from, serial, error::MATCH_RULE_INVALID, b"Invalid match rule");
                };
                let Some(Some(connection)) = self.connections.get_mut(from) else {
                    return;
                };
                match connection.rules.iter().position(|held| *held == rule) {
                    Some(at) => {
                        connection.rules.remove(at);
                        self.reply_to(from, serial, b"", &out);
                    }
                    None => self.error_to(from, serial, error::MATCH_RULE_NOT_FOUND, b"The rule was not added"),
                }
            }
            (BUS_NAME, b"GetId", b"") | (b"org.freedesktop.DBus.Peer", b"GetMachineId", b"") => {
                let id = self.id;
                out.string(&id);
                self.reply_to(from, serial, b"s", &out);
            }
            (BUS_NAME, b"StartServiceByName", b"su") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                let name = name.to_vec();
                match self.owner(&name) {
                    Some(_) => {
                        out.u32(2);
                        self.reply_to(from, serial, b"u", &out);
                    }
                    None => {
                        let mut text = Vec::from(&b"The name "[..]);
                        text.extend_from_slice(&name);
                        text.extend_from_slice(b" was not provided by any .service files");
                        self.error_to(from, serial, error::SERVICE_UNKNOWN, &text);
                    }
                }
            }
            (BUS_NAME, b"UpdateActivationEnvironment", b"a{ss}") => {
                self.error_to(from, serial, error::ACCESS_DENIED, b"Activation is not supported");
            }
            (BUS_NAME, b"GetConnectionUnixUser" | b"GetConnectionUnixProcessID" | b"GetConnectionCredentials", b"s") => {
                let Some(name) = first_string() else {
                    return self.invalid(from, serial);
                };
                let name = name.to_vec();
                let Some(credentials) = self.owner(&name).and_then(|owner| self.connection(owner)).map(|connection| connection.credentials) else {
                    return self.no_owner(from, serial, &name);
                };
                match member {
                    b"GetConnectionUnixUser" => {
                        out.u32(credentials.uid);
                        self.reply_to(from, serial, b"u", &out);
                    }
                    b"GetConnectionUnixProcessID" => {
                        out.u32(credentials.pid);
                        self.reply_to(from, serial, b"u", &out);
                    }
                    _ => {
                        let array = out.begin_array(8);
                        for (key, value) in [(&b"UnixUserID"[..], credentials.uid), (b"ProcessID", credentials.pid)] {
                            out.align(8);
                            out.string(key);
                            out.signature(b"u");
                            out.u32(value);
                        }
                        out.end_array(array);
                        self.reply_to(from, serial, b"a{sv}", &out);
                    }
                }
            }
            (b"org.freedesktop.DBus.Peer", b"Ping", b"") => self.reply_to(from, serial, b"", &out),
            (b"org.freedesktop.DBus.Introspectable", b"Introspect", b"") => {
                out.string(INTROSPECTION);
                self.reply_to(from, serial, b"s", &out);
            }
            _ => {
                if message.expects_reply() {
                    let mut text = Vec::from(&b"No method "[..]);
                    text.extend_from_slice(member);
                    text.extend_from_slice(b" with signature \"");
                    text.extend_from_slice(signature);
                    text.extend_from_slice(b"\" on the bus");
                    self.error_to(from, serial, error::UNKNOWN_METHOD, &text);
                }
            }
        }
    }

    fn invalid(&mut self, to: ConnectionId, serial: u32) {
        self.error_to(to, serial, error::INVALID_ARGS, b"Invalid arguments");
    }

    fn no_owner(&mut self, to: ConnectionId, serial: u32, name: &[u8]) {
        let mut text = Vec::from(&b"The name "[..]);
        text.extend_from_slice(name);
        text.extend_from_slice(b" has no owner");
        self.error_to(to, serial, error::NAME_HAS_NO_OWNER, &text);
    }

    fn request_name(&mut self, from: ConnectionId, name: &[u8], flags: u32) -> Option<(u32, Vec<u8>)> {
        let held = self.connection(from).map_or(0, |connection| connection.names);
        let Some(queue) = self.names.get_mut(name) else {
            if held >= MAX_NAMES_PER_CONNECTION {
                return None;
            }
            self.names.insert(name.to_vec(), Vec::from([(from, flags)]));
            self.count_names(from, 1);
            return Some((request::PRIMARY_OWNER, Vec::new()));
        };
        let (primary, primary_flags) = queue[0];
        if primary == from {
            queue[0].1 = flags;
            return Some((request::ALREADY_OWNER, Vec::new()));
        }
        let queued = queue.iter().position(|&(owner, _)| owner == from);
        let replaces = primary_flags & request::ALLOW_REPLACEMENT != 0 && flags & request::REPLACE_EXISTING != 0;
        if replaces {
            if let Some(at) = queued {
                queue.remove(at);
            }
            queue[0] = (from, flags);
            let old_stays = primary_flags & request::DO_NOT_QUEUE == 0;
            if old_stays {
                queue.insert(1, (primary, primary_flags));
            }
            if queued.is_none() {
                self.count_names(from, 1);
            }
            if !old_stays {
                self.count_names(primary, -1);
            }
            let previous = self.unique(primary).to_vec();
            self.signal_to(primary, b"NameLost", name);
            return Some((request::PRIMARY_OWNER, previous));
        }
        if flags & request::DO_NOT_QUEUE != 0 {
            if let Some(at) = queued {
                queue.remove(at);
                self.count_names(from, -1);
            }
            return Some((request::EXISTS, Vec::new()));
        }
        match queued {
            Some(at) => queue[at].1 = flags,
            None => {
                if held >= MAX_NAMES_PER_CONNECTION {
                    return None;
                }
                queue.push((from, flags));
                self.count_names(from, 1);
            }
        }
        Some((request::IN_QUEUE, Vec::new()))
    }

    fn after_new_owner(&mut self, name: &[u8], owner: ConnectionId, previous: &[u8]) {
        let new = self.unique(owner).to_vec();
        self.name_owner_changed(name, previous, &new);
        self.signal_to(owner, b"NameAcquired", name);
    }

    fn count_names(&mut self, id: ConnectionId, change: isize) {
        if let Some(Some(connection)) = self.connections.get_mut(id) {
            connection.names = connection.names.saturating_add_signed(change);
        }
    }

    fn release(&mut self, name: &[u8], id: ConnectionId, unique: &[u8], tell: bool) -> u32 {
        let Some(queue) = self.names.get_mut(name) else {
            return release::NON_EXISTENT;
        };
        let Some(at) = queue.iter().position(|&(owner, _)| owner == id) else {
            return release::NOT_OWNER;
        };
        queue.remove(at);
        let next = queue.first().map(|&(owner, _)| owner);
        if queue.is_empty() {
            self.names.remove(name);
        }
        self.count_names(id, -1);
        if at != 0 {
            return release::RELEASED;
        }
        if tell {
            self.signal_to(id, b"NameLost", name);
        }
        match next {
            Some(next) => {
                let new = self.unique(next).to_vec();
                self.name_owner_changed(name, unique, &new);
                self.signal_to(next, b"NameAcquired", name);
            }
            None => self.name_owner_changed(name, unique, b""),
        }
        release::RELEASED
    }
}

fn string_array(out: &mut Writer, strings: &[Vec<u8>]) {
    let array = out.begin_array(4);
    for string in strings {
        out.string(string);
    }
    out.end_array(array);
}

#[cfg(test)]
mod tests;
