use crate::options::parse_count;
use crate::output::{FAILURE, Output};
use crate::system::{DatagramKind, System};
use core::ffi::CStr;
use eggos_net::Ipv4Address;
use eggos_net::dns::{self, Answers, QueryError, ResponseError};
use eggos_net::status::{InterfaceStatus, parse_interfaces};
use eggos_runtime::errno::Errno;

pub const INTERFACES: &CStr = c"/proc/net/interfaces";
pub const NEIGHBOURS: &CStr = c"/proc/net/arp";
pub const RESOLV_CONF: &CStr = c"/etc/resolv.conf";
pub const MAX_INTERFACES: usize = 8;
pub const TEXT_CAPACITY: usize = 4096;
pub const DNS_TIMEOUT_MS: u32 = 2000;
pub const DNS_ATTEMPTS: u32 = 2;

pub struct Text {
    pub bytes: [u8; TEXT_CAPACITY],
    pub length: usize,
}

impl Text {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

pub fn read_text<S: System>(system: &mut S, path: &CStr) -> Result<Text, Errno> {
    let descriptor = system.open(path)?;
    let mut text = Text {
        bytes: [0; TEXT_CAPACITY],
        length: 0,
    };
    let read = loop {
        if text.length == TEXT_CAPACITY {
            break Ok(());
        }
        match system.read(descriptor, &mut text.bytes[text.length..]) {
            Ok(0) => break Ok(()),
            Ok(count) => text.length += count,
            Err(Errno::EINTR) => {}
            Err(errno) => break Err(errno),
        }
    };
    system.close(descriptor);
    read.map(|()| text)
}

pub type Interfaces = ([Option<InterfaceStatus>; MAX_INTERFACES], usize);

pub fn read_interfaces<S: System>(output: &mut Output<S>) -> Result<Interfaces, i32> {
    match read_text(&mut output.system, INTERFACES) {
        Ok(text) => Ok(parse_interfaces::<MAX_INTERFACES>(text.as_bytes())),
        Err(errno) => {
            output.file_error(INTERFACES, errno);
            Err(FAILURE)
        }
    }
}

pub fn interfaces(read: &Interfaces) -> impl Iterator<Item = &InterfaceStatus> {
    read.0[..read.1].iter().flatten()
}

pub fn nameservers<S: System>(system: &mut S) -> [Option<Ipv4Address>; dns::MAX_NAMESERVERS] {
    match read_text(system, RESOLV_CONF) {
        Ok(text) => dns::nameservers(text.as_bytes()),
        Err(_) => [None; dns::MAX_NAMESERVERS],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupError {
    BadName(QueryError),
    TimedOut,
    Answer(ResponseError),
    System(Errno),
}

impl LookupError {
    pub fn message(self) -> &'static [u8] {
        match self {
            Self::BadName(QueryError::LabelTooLong) => {
                b"a part of the name is longer than 63 letters"
            }
            Self::BadName(QueryError::NameTooLong) => b"the name is longer than 253 letters",
            Self::BadName(_) => b"the name is empty or has an empty part",
            Self::TimedOut => b"no DNS server answered",
            Self::Answer(error) => error.description().as_bytes(),
            Self::System(errno) => errno.message(),
        }
    }
}

pub fn lookup<S: System>(
    system: &mut S,
    name: &[u8],
    server: Ipv4Address,
    port: u16,
    timeout_ms: u32,
) -> Result<Answers, LookupError> {
    let descriptor = system
        .open_datagram_socket(DatagramKind::Udp)
        .map_err(LookupError::System)?;
    let result = exchange(system, descriptor, name, server, port, timeout_ms);
    system.close(descriptor);
    result
}

fn exchange<S: System>(
    system: &mut S,
    descriptor: usize,
    name: &[u8],
    server: Ipv4Address,
    port: u16,
    timeout_ms: u32,
) -> Result<Answers, LookupError> {
    let id = (system.monotonic_us() as u16) ^ 0x5eed;
    let mut query = [0u8; dns::MAX_MESSAGE_BYTES];
    let length = dns::write_query(&mut query, id, name, dns::record_type::A)
        .map_err(LookupError::BadName)?;
    let mut answer = [0u8; dns::MAX_MESSAGE_BYTES];
    for _ in 0..DNS_ATTEMPTS {
        system
            .send_to(descriptor, &query[..length], server, port)
            .map_err(LookupError::System)?;
        let started = system.monotonic_us();
        loop {
            let waited_ms = (system.monotonic_us().saturating_sub(started) / 1000) as u32;
            let Some(left) = timeout_ms.checked_sub(waited_ms).filter(|&left| left > 0) else {
                break;
            };
            let received = system
                .receive_from(descriptor, &mut answer, left)
                .map_err(LookupError::System)?;
            let Some((count, sender)) = received else {
                break;
            };
            if sender.address != server || sender.port != port {
                continue;
            }
            match dns::parse_response(&answer[..count], id) {
                Err(ResponseError::NotOurs) => continue,
                result => return result.map_err(LookupError::Answer),
            }
        }
    }
    Err(LookupError::TimedOut)
}

pub fn resolve<S: System>(output: &mut Output<S>, name: &[u8]) -> Result<Ipv4Address, i32> {
    if let Some(address) = Ipv4Address::parse(name) {
        return Ok(address);
    }
    if name == b"localhost" {
        return Ok(Ipv4Address::LOOPBACK);
    }
    let servers = nameservers(&mut output.system);
    let mut failure = LookupError::TimedOut;
    for server in servers.into_iter().flatten() {
        match lookup(&mut output.system, name, server, dns::PORT, DNS_TIMEOUT_MS) {
            Ok(answers) => match answers.addresses().first() {
                Some(&address) => return Ok(address),
                None => failure = LookupError::Answer(ResponseError::NameNotFound),
            },
            Err(error @ LookupError::Answer(ResponseError::NameNotFound)) => {
                failure = error;
                break;
            }
            Err(error) => failure = error,
        }
    }
    if servers.iter().all(Option::is_none) {
        output.error(&[name, b": no DNS server: /etc/resolv.conf names none"]);
    } else {
        output.error(&[name, b": ", failure.message()]);
    }
    Err(FAILURE)
}

pub struct Shown<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Shown<N> {
    pub fn of(value: impl core::fmt::Display) -> Self {
        let mut shown = Self {
            bytes: [0; N],
            length: 0,
        };
        let _ = core::fmt::write(&mut shown, format_args!("{value}"));
        shown
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl<const N: usize> core::fmt::Write for Shown<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let room = self
            .bytes
            .get_mut(self.length..end)
            .ok_or(core::fmt::Error)?;
        room.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

pub fn milliseconds(microseconds: u64) -> Shown<24> {
    Shown::of(format_args!(
        "{}.{:03}",
        microseconds / 1000,
        microseconds % 1000
    ))
}

pub fn parse_seconds_ms(text: &[u8]) -> Option<u64> {
    let (whole, fraction) = match text.iter().position(|&byte| byte == b'.') {
        Some(dot) => (&text[..dot], &text[dot + 1..]),
        None => (text, &b""[..]),
    };
    let whole = match whole {
        b"" => 0,
        whole => parse_count(whole)?,
    };
    if fraction.len() > 3 || (fraction.is_empty() && text.ends_with(b".")) {
        return None;
    }
    let mut milliseconds = 0;
    for (index, digit) in fraction.iter().enumerate() {
        if !digit.is_ascii_digit() {
            return None;
        }
        milliseconds += u64::from(digit - b'0') * [100, 10, 1][index];
    }
    whole.checked_mul(1000)?.checked_add(milliseconds)
}
