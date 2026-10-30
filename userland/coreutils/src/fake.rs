use crate::system::{
    CapturedRows, DIRECTORY, DatagramKind, Duration, FILE_TYPE, FileSystemUsage, MAX_LINK_HOPS,
    STANDARD_ERROR, STANDARD_INPUT, STANDARD_OUTPUT, SYMBOLIC_LINK, Sender, Status, SymbolicLinks,
    System, SystemName, UTSNAME_FIELD, UTSNAME_SIZE, WindowTree,
};
use core::ffi::CStr;
use eggos_net::{Ipv4Address, dns, icmp, sntp};
use eggos_runtime::errno::Errno;
use std::collections::{BTreeMap, VecDeque};

const FIRST_SOCKET_DESCRIPTOR: usize = 200;
const FIRST_SOCKET_PORT: u16 = 40_000;
const FIRST_STREAM_DESCRIPTOR: usize = 300;
const STREAM_PIECE: usize = 7;

#[derive(Debug, Clone)]
pub struct FakePage {
    pub address: Ipv4Address,
    pub port: u16,
    pub target: Vec<u8>,
    pub response: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct FakeStream {
    pub address: Option<(Ipv4Address, u16)>,
    pub request: Vec<u8>,
    pub response: VecDeque<u8>,
    pub answered: bool,
    pub open: bool,
}

#[derive(Debug)]
pub struct FakeSocket {
    pub kind: DatagramKind,
    pub port: u16,
    pub queue: VecDeque<(Vec<u8>, Sender)>,
    pub open: bool,
}

#[derive(Debug, Default)]
pub struct FakeNetwork {
    pub reachable: Vec<Ipv4Address>,
    pub names: Vec<(Vec<u8>, Ipv4Address)>,
    pub dns_servers: Vec<(Ipv4Address, u16)>,
    pub round_trip_us: u64,
    pub sent: Vec<(DatagramKind, Ipv4Address, u16, Vec<u8>)>,
    pub sockets: Vec<FakeSocket>,
    pub monotonic_us: u64,
    pub send_error: Option<Errno>,
    pub pages: Vec<FakePage>,
    pub streams: Vec<FakeStream>,
    pub requests: Vec<Vec<u8>>,
    pub raw_answers: Vec<((Ipv4Address, u16), Vec<u8>)>,
    pub ntp_servers: Vec<(Ipv4Address, u16)>,
    pub ntp_time_ns: i64,
    pub random_counter: u8,
}

impl FakeNetwork {
    fn stream(&mut self, descriptor: usize) -> Result<&mut FakeStream, Errno> {
        self.streams
            .get_mut(descriptor.wrapping_sub(FIRST_STREAM_DESCRIPTOR))
            .filter(|stream| stream.open)
            .ok_or(Errno(9))
    }

    fn page(&self, address: (Ipv4Address, u16), request: &[u8]) -> Vec<u8> {
        let target = request
            .strip_prefix(b"GET ")
            .and_then(|rest| rest.split(|&byte| byte == b' ').next())
            .unwrap_or_default();
        self.pages
            .iter()
            .find(|page| (page.address, page.port) == address && page.target == target)
            .map_or_else(
                || b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found".to_vec(),
                |page| page.response.clone(),
            )
    }

    fn socket(&mut self, descriptor: usize) -> Result<&mut FakeSocket, Errno> {
        self.sockets
            .get_mut(descriptor.wrapping_sub(FIRST_SOCKET_DESCRIPTOR))
            .filter(|socket| socket.open)
            .ok_or(Errno(9))
    }

    fn answer(&mut self, descriptor: usize, bytes: &[u8], address: Ipv4Address, port: u16) {
        let Ok(socket) = self.socket(descriptor) else {
            return;
        };
        let kind = socket.kind;
        let identifier = socket.port;
        let reply = match kind {
            DatagramKind::Ping if self.reachable.contains(&address) => {
                let mut reply = bytes.to_vec();
                let sequence = u16::from_be_bytes([bytes[6], bytes[7]]);
                let data = bytes.len() - icmp::HEADER_BYTES;
                icmp::write_echo_header(
                    &mut reply,
                    icmp::kind::ECHO_REPLY,
                    identifier,
                    sequence,
                    data,
                );
                Some((reply, Sender { address, port: 0 }))
            }
            DatagramKind::Udp if self.ntp_servers.contains(&(address, port)) => {
                let time = sntp::Timestamp::from_unix_nanoseconds(self.ntp_time_ns);
                let mut reply = [0u8; sntp::PACKET_BYTES];
                if !sntp::write_reply(bytes, time, time, &mut reply) {
                    return;
                }
                Some((reply.to_vec(), Sender { address, port }))
            }
            DatagramKind::Udp if self.dns_servers.contains(&(address, port)) => {
                let Some(name) = dns::query_name(bytes) else {
                    return;
                };
                let addresses: Vec<Ipv4Address> = self
                    .names
                    .iter()
                    .filter(|(known, _)| known.as_slice() == name.as_bytes())
                    .map(|&(_, address)| address)
                    .collect();
                let mut answer = [0u8; dns::MAX_MESSAGE_BYTES];
                let length = dns::write_answer(&mut answer, bytes, &addresses, 60).unwrap();
                Some((answer[..length].to_vec(), Sender { address, port }))
            }
            _ => None,
        };
        if let (Some(reply), Ok(socket)) = (reply, self.socket(descriptor)) {
            socket.queue.push_back(reply);
        }
    }
}

const REGULAR_FILE: u32 = 0o100_000;
const FIRST_FILE_DESCRIPTOR: usize = 3;

#[derive(Debug, Clone)]
pub enum Node {
    Directory,
    File(Vec<u8>),
    Link(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct FakeWindow {
    pub id: u32,
    pub state: u32,
    pub rect: [u32; 4],
    pub client: u32,
    pub title: Vec<u8>,
}

#[derive(Debug, Clone)]
struct OpenFile {
    path: Vec<u8>,
    position: usize,
}

#[derive(Debug)]
pub struct Fake {
    pub nodes: BTreeMap<Vec<u8>, (Node, Status)>,
    pub standard_input: Vec<u8>,
    input_position: usize,
    pub standard_output: Vec<u8>,
    pub standard_error: Vec<u8>,
    pub write_error: Option<Errno>,
    pub status_error: Option<(Vec<u8>, Errno)>,
    pub name: SystemName,
    pub slept: Vec<Duration>,
    pub rebooted: Vec<u32>,
    pub synced: usize,
    pub reboot_error: Option<Errno>,
    pub init_signals: Vec<u32>,
    pub init_signal_error: Option<Errno>,
    pub terminal_width: Option<u16>,
    pub cross_device: Vec<Vec<u8>>,
    pub now: i64,
    pub now_nanoseconds: u32,
    pub set_times: Vec<Duration>,
    pub set_time_error: Option<Errno>,
    pub windows: Vec<FakeWindow>,
    pub managed: Vec<[u32; 4]>,
    pub window_pixels: BTreeMap<u32, (u32, u32, Vec<u32>)>,
    pub sent_input: Vec<[u32; 6]>,
    pub input_results: Vec<Option<Errno>>,
    pub dialogs: Vec<(u32, u32)>,
    pub menus: BTreeMap<u32, Vec<u8>>,
    pub chosen_menus: Vec<[u32; 3]>,
    pub chosen_labels: Vec<Vec<u8>>,
    pub trees: BTreeMap<u32, Vec<u8>>,
    pub acted: Vec<(u32, u32, u32, Vec<u8>)>,
    pub act_outcome: u32,
    pub capture_calls: usize,
    pub capture_fails_at: Option<(usize, Errno)>,
    pub resized_at: Option<(usize, u32, u32)>,
    pub window_server: bool,
    pub working_directory: String,
    pub network: FakeNetwork,
    pub file_systems: Vec<(Vec<u8>, FileSystemUsage)>,
    open: Vec<Option<OpenFile>>,
    next_inode: u64,
}

impl Default for Fake {
    fn default() -> Self {
        let mut name = [0u8; UTSNAME_SIZE];
        for (index, value) in [
            &b"Linux"[..],
            b"localhost",
            b"6.1.0-eggos",
            b"#1 SMP EggOS 0.2.0",
            b"aarch64",
            b"(none)",
        ]
        .into_iter()
        .enumerate()
        {
            name[index * UTSNAME_FIELD..][..value.len()].copy_from_slice(value);
        }
        Self {
            nodes: BTreeMap::new(),
            standard_input: Vec::new(),
            input_position: 0,
            standard_output: Vec::new(),
            standard_error: Vec::new(),
            write_error: None,
            status_error: None,
            name: SystemName(name),
            slept: Vec::new(),
            rebooted: Vec::new(),
            synced: 0,
            reboot_error: Some(Errno::EPERM),
            init_signals: Vec::new(),
            init_signal_error: None,
            terminal_width: None,
            cross_device: Vec::new(),
            now: 1_700_000_000,
            now_nanoseconds: 0,
            set_times: Vec::new(),
            set_time_error: None,
            windows: Vec::new(),
            managed: Vec::new(),
            window_pixels: BTreeMap::new(),
            sent_input: Vec::new(),
            input_results: Vec::new(),
            dialogs: Vec::new(),
            menus: BTreeMap::new(),
            chosen_menus: Vec::new(),
            chosen_labels: Vec::new(),
            trees: BTreeMap::new(),
            acted: Vec::new(),
            act_outcome: 0,
            capture_calls: 0,
            capture_fails_at: None,
            resized_at: None,
            window_server: true,
            working_directory: "/".to_owned(),
            network: FakeNetwork::default(),
            file_systems: Vec::new(),
            open: Vec::new(),
            next_inode: 1,
        }
    }
}

impl Fake {
    fn inode(&mut self) -> u64 {
        self.next_inode += 1;
        self.next_inode
    }

    pub fn with_directory(mut self, path: &str, modified: i64) -> Self {
        let status = Status {
            mode: DIRECTORY | 0o755,
            links: 2,
            size: 0,
            blocks: 0,
            modified,
            inode: self.inode(),
            ..Status::default()
        };
        self.nodes
            .insert(path.as_bytes().to_vec(), (Node::Directory, status));
        self
    }

    pub fn with_file(mut self, path: &str, mode: u32, content: &[u8], modified: i64) -> Self {
        let status = Status {
            mode: REGULAR_FILE | mode,
            links: 1,
            size: content.len() as u64,
            blocks: (content.len() as u64).div_ceil(512),
            modified,
            inode: self.inode(),
            ..Status::default()
        };
        self.nodes.insert(
            path.as_bytes().to_vec(),
            (Node::File(content.to_vec()), status),
        );
        self
    }

    pub fn with_link(mut self, path: &str, target: &str) -> Self {
        let status = self.link_status(target.as_bytes());
        self.nodes.insert(
            path.as_bytes().to_vec(),
            (Node::Link(target.as_bytes().to_vec()), status),
        );
        self
    }

    fn link_status(&mut self, target: &[u8]) -> Status {
        Status {
            mode: SYMBOLIC_LINK | 0o777,
            links: 1,
            size: target.len() as u64,
            modified: self.now,
            inode: self.inode(),
            ..Status::default()
        }
    }

    pub fn link_target(&self, path: &str) -> Option<Vec<u8>> {
        match self.nodes.get(path.as_bytes()) {
            Some((Node::Link(target), _)) => Some(target.clone()),
            _ => None,
        }
    }

    fn through_links(&self, path: &[u8], follow_last: bool) -> Result<Vec<u8>, Errno> {
        if path.first() != Some(&b'/') {
            return Ok(path.to_vec());
        }
        let mut done: Vec<u8> = Vec::new();
        let mut rest: Vec<Vec<u8>> = path
            .split(|&byte| byte == b'/')
            .filter(|part| !part.is_empty())
            .rev()
            .map(<[u8]>::to_vec)
            .collect();
        let mut hops = 0;
        while let Some(part) = rest.pop() {
            match part.as_slice() {
                b"." => continue,
                b".." => {
                    let cut = done.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
                    done.truncate(cut);
                    continue;
                }
                _ => {}
            }
            let next = [done.as_slice(), b"/", part.as_slice()].concat();
            let last = rest.is_empty();
            match self.nodes.get(&next) {
                Some((Node::Link(target), _)) if follow_last || !last => {
                    hops += 1;
                    if hops > MAX_LINK_HOPS {
                        return Err(Errno::ELOOP);
                    }
                    if target.first() == Some(&b'/') {
                        done.clear();
                    }
                    rest.extend(
                        target
                            .split(|&byte| byte == b'/')
                            .filter(|piece| !piece.is_empty())
                            .rev()
                            .map(<[u8]>::to_vec),
                    );
                }
                Some((Node::File(_), _)) if !last && hops > 0 => return Err(Errno::ENOTDIR),
                None if !last && hops > 0 => return Err(Errno::ENOENT),
                _ => done = next,
            }
        }
        match (hops, done.is_empty()) {
            (0, _) => Ok(path.to_vec()),
            (_, true) => Ok(b"/".to_vec()),
            (_, false) => Ok(done),
        }
    }

    pub fn contents(&self, path: &str) -> Vec<u8> {
        match self.nodes.get(path.as_bytes()) {
            Some((Node::File(content), _)) => content.clone(),
            _ => panic!("{path} is not a file"),
        }
    }

    pub fn open_count(&self) -> usize {
        self.open.iter().filter(|file| file.is_some()).count()
    }

    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.standard_output).into_owned()
    }

    pub fn errors(&self) -> String {
        String::from_utf8_lossy(&self.standard_error).into_owned()
    }

    fn children(&self, directory: &[u8]) -> Vec<(Vec<u8>, bool)> {
        let prefix = match directory {
            b"/" => b"/".to_vec(),
            _ => [directory, b"/"].concat(),
        };
        let mut children: Vec<(Vec<u8>, bool)> =
            vec![(b".".to_vec(), true), (b"..".to_vec(), true)];
        children.extend(self.nodes.iter().rev().filter_map(|(path, (node, _))| {
            let name = path.strip_prefix(prefix.as_slice())?;
            (!name.is_empty() && !name.contains(&b'/'))
                .then(|| (name.to_vec(), matches!(node, Node::Directory)))
        }));
        children
    }

    fn file(&mut self, descriptor: usize) -> Result<&mut OpenFile, Errno> {
        self.open
            .get_mut(descriptor.wrapping_sub(FIRST_FILE_DESCRIPTOR))
            .and_then(Option::as_mut)
            .ok_or(Errno(9))
    }
}

fn split_last_component(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => (b"/", &path[1..]),
        Some(slash) => (&path[..slash], &path[slash + 1..]),
        None => (b".", path),
    }
}

fn parent(directory: &[u8]) -> Vec<u8> {
    match directory.iter().rposition(|&byte| byte == b'/') {
        Some(0) | None => b"/".to_vec(),
        Some(slash) => directory[..slash].to_vec(),
    }
}

impl System for Fake {
    fn write(&mut self, descriptor: usize, bytes: &[u8]) -> Result<(), Errno> {
        match descriptor {
            STANDARD_OUTPUT => match self.write_error {
                Some(errno) => Err(errno),
                None => {
                    self.standard_output.extend_from_slice(bytes);
                    Ok(())
                }
            },
            STANDARD_ERROR => {
                self.standard_error.extend_from_slice(bytes);
                Ok(())
            }
            _ => {
                let file = self.file(descriptor)?.clone();
                let Some((Node::File(content), status)) = self.nodes.get_mut(&file.path) else {
                    return Err(Errno(9));
                };
                let end = file.position + bytes.len();
                if content.len() < end {
                    content.resize(end, 0);
                }
                content[file.position..end].copy_from_slice(bytes);
                status.size = content.len() as u64;
                self.file(descriptor)?.position = end;
                Ok(())
            }
        }
    }

    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
        if descriptor == STANDARD_INPUT {
            let rest = &self.standard_input[self.input_position..];
            let length = rest.len().min(buffer.len()).min(3);
            buffer[..length].copy_from_slice(&rest[..length]);
            self.input_position += length;
            return Ok(length);
        }
        let file = self.file(descriptor)?.clone();
        let Some((Node::File(content), _)) = self.nodes.get(&file.path) else {
            return Err(Errno::EISDIR);
        };
        let rest = &content[file.position.min(content.len())..];
        let length = rest.len().min(buffer.len());
        buffer[..length].copy_from_slice(&rest[..length]);
        self.file(descriptor)?.position += length;
        Ok(length)
    }

    fn open(&mut self, path: &CStr) -> Result<usize, Errno> {
        let window_device = path == crate::wm::DEVICE;
        if window_device && !self.window_server {
            return Err(Errno::ENXIO);
        }
        let mut path = self.resolve(path.to_bytes());
        if !window_device {
            path = self.through_links(&path, true)?;
            if !self.nodes.contains_key(&path) {
                return Err(Errno::ENOENT);
            }
        }
        self.open.push(Some(OpenFile { path, position: 0 }));
        Ok(self.open.len() - 1 + FIRST_FILE_DESCRIPTOR)
    }

    fn close(&mut self, descriptor: usize) {
        if let Ok(stream) = self.network.stream(descriptor) {
            stream.open = false;
            return;
        }
        if let Ok(socket) = self.network.socket(descriptor) {
            socket.open = false;
            return;
        }
        if let Some(file) = self.open.get_mut(descriptor - FIRST_FILE_DESCRIPTOR) {
            *file = None;
        }
    }

    fn directory_entries(&mut self, descriptor: usize, buffer: &mut [u8]) -> Result<usize, Errno> {
        let file = self.file(descriptor)?.clone();
        if !matches!(self.nodes.get(&file.path), Some((Node::Directory, _))) {
            return Err(Errno::ENOTDIR);
        }
        let mut written = 0;
        let mut position = file.position;
        for (name, directory) in self.children(&file.path).into_iter().skip(file.position) {
            let length = (19 + name.len() + 1).next_multiple_of(8);
            if written + length > buffer.len() {
                if written == 0 {
                    return Err(Errno::EINVAL);
                }
                break;
            }
            let record = &mut buffer[written..written + length];
            record.fill(0);
            record[..8].copy_from_slice(&(position as u64 + 1).to_ne_bytes());
            record[8..16].copy_from_slice(&(position as u64 + 1).to_ne_bytes());
            record[16..18].copy_from_slice(&(length as u16).to_ne_bytes());
            record[18] = if directory { 4 } else { 8 };
            record[19..19 + name.len()].copy_from_slice(&name);
            written += length;
            position += 1;
        }
        self.file(descriptor)?.position = position;
        Ok(written)
    }

    fn status(
        &mut self,
        directory: Option<usize>,
        path: &CStr,
        links: SymbolicLinks,
    ) -> Result<Status, Errno> {
        let path = match directory {
            Some(descriptor) => {
                let directory = self.file(descriptor)?.path.clone();
                match directory.as_slice() {
                    b"/" => [b"/", path.to_bytes()].concat(),
                    _ => [&directory[..], b"/", path.to_bytes()].concat(),
                }
            }
            None => self.resolve(path.to_bytes()),
        };
        let path = match split_last_component(&path) {
            (directory, b".") => directory.to_vec(),
            (directory, b"..") => parent(directory),
            _ => path,
        };
        if let Some((failing, errno)) = &self.status_error
            && *failing == path
        {
            return Err(*errno);
        }
        let path = self.through_links(&path, links == SymbolicLinks::Follow)?;
        self.nodes
            .get(&path)
            .map(|(_, status)| *status)
            .ok_or(Errno::ENOENT)
    }

    fn system_name(&mut self) -> Result<SystemName, Errno> {
        Ok(self.name.clone())
    }

    fn sleep(&mut self, duration: Duration) -> Result<(), Errno> {
        self.slept.push(duration);
        self.network.monotonic_us +=
            duration.seconds * 1_000_000 + u64::from(duration.nanoseconds) / 1000;
        Ok(())
    }

    fn sync(&mut self) {
        self.synced += 1;
    }

    fn reboot(&mut self, command: u32) -> Result<(), Errno> {
        assert!(
            self.synced > 0,
            "changes are synced before the machine goes down"
        );
        self.rebooted.push(command);
        self.reboot_error.map_or(Ok(()), Err)
    }

    fn create(&mut self, path: &CStr, mode: u32, truncate: bool) -> Result<usize, Errno> {
        let bytes = self.through_links(&self.resolve(path.to_bytes()), true)?;
        match self.nodes.get_mut(&bytes) {
            Some((Node::Directory | Node::Link(_), _)) => return Err(Errno::EISDIR),
            Some((Node::File(content), status)) => {
                if truncate {
                    content.clear();
                    status.size = 0;
                }
            }
            None => {
                self.parent_directory(&bytes)?;
                let text = String::from_utf8(bytes.clone()).unwrap();
                *self = std::mem::take(self).with_file(&text, mode & 0o7777, b"", 0);
            }
        }
        self.open.push(Some(OpenFile {
            path: bytes,
            position: 0,
        }));
        Ok(self.open.len() - 1 + FIRST_FILE_DESCRIPTOR)
    }

    fn append(&mut self, path: &CStr, mode: u32) -> Result<usize, Errno> {
        let descriptor = self.create(path, mode, false)?;
        let opened = self.file(descriptor)?.path.clone();
        let end = match self.nodes.get(&opened) {
            Some((Node::File(content), _)) => content.len(),
            _ => 0,
        };
        self.file(descriptor)?.position = end;
        Ok(descriptor)
    }

    fn make_directory(&mut self, path: &CStr, _mode: u32) -> Result<(), Errno> {
        let bytes = self.resolve(path.to_bytes());
        if self.nodes.contains_key(&bytes) {
            return Err(Errno::EEXIST);
        }
        self.parent_directory(&bytes)?;
        let text = String::from_utf8(bytes).unwrap();
        *self = std::mem::take(self).with_directory(&text, 0);
        Ok(())
    }

    fn remove(&mut self, path: &CStr, directory: bool) -> Result<(), Errno> {
        let bytes = self.resolve(path.to_bytes());
        match (self.nodes.get(&bytes), directory) {
            (None, _) => Err(Errno::ENOENT),
            (Some((Node::Directory, _)), false) => Err(Errno::EISDIR),
            (Some((Node::File(_) | Node::Link(_), _)), true) => Err(Errno::ENOTDIR),
            (Some((Node::Directory, _)), true) if self.children(&bytes).len() > 2 => {
                Err(Errno::ENOTEMPTY)
            }
            _ => {
                self.nodes.remove(&bytes);
                Ok(())
            }
        }
    }

    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno> {
        let (from, to) = (self.resolve(from.to_bytes()), self.resolve(to.to_bytes()));
        if self
            .cross_device
            .iter()
            .any(|device| to.starts_with(device) != from.starts_with(device))
        {
            return Err(Errno::EXDEV);
        }
        let node = self.nodes.remove(&from).ok_or(Errno::ENOENT)?;
        let moved: Vec<(Vec<u8>, (Node, Status))> = self
            .nodes
            .iter()
            .filter(|(path, _)| path.starts_with(&from) && path.get(from.len()) == Some(&b'/'))
            .map(|(path, node)| (path.clone(), node.clone()))
            .collect();
        for (path, inner) in moved {
            self.nodes.remove(&path);
            self.nodes
                .insert([&to[..], &path[from.len()..]].concat(), inner);
        }
        self.nodes.insert(to, node);
        Ok(())
    }

    fn set_modified_now(&mut self, path: &CStr) -> Result<(), Errno> {
        let (_, status) = self.nodes.get_mut(path.to_bytes()).ok_or(Errno::ENOENT)?;
        status.modified = self.now;
        Ok(())
    }

    fn read_link(&mut self, path: &CStr, target: &mut [u8]) -> Result<usize, Errno> {
        let path = self.through_links(&self.resolve(path.to_bytes()), false)?;
        match self.nodes.get(&path) {
            Some((Node::Link(text), _)) => {
                let length = text.len().min(target.len());
                target[..length].copy_from_slice(&text[..length]);
                Ok(length)
            }
            Some(_) => Err(Errno::EINVAL),
            None => Err(Errno::ENOENT),
        }
    }

    fn make_link(&mut self, target: &CStr, path: &CStr) -> Result<(), Errno> {
        let path = self.through_links(&self.resolve(path.to_bytes()), false)?;
        if target.is_empty() {
            return Err(Errno::ENOENT);
        }
        if self.nodes.contains_key(&path) {
            return Err(Errno::EEXIST);
        }
        self.parent_directory(&path)?;
        let status = self.link_status(target.to_bytes());
        let link = Node::Link(target.to_bytes().to_vec());
        self.nodes.insert(path, (link, status));
        Ok(())
    }

    fn file_system_usage(&mut self, path: &CStr) -> Result<FileSystemUsage, Errno> {
        let path = self.through_links(&self.resolve(path.to_bytes()), true)?;
        if !self.nodes.contains_key(&path) && path != b"/" {
            return Err(Errno::ENOENT);
        }
        let holds = |point: &[u8]| {
            point == b"/"
                || path == point
                || path
                    .strip_prefix(point)
                    .is_some_and(|rest| rest.starts_with(b"/"))
        };
        self.file_systems
            .iter()
            .filter(|(point, _)| holds(point))
            .max_by_key(|(point, _)| point.len())
            .map(|&(_, usage)| usage)
            .ok_or(Errno(38))
    }

    fn set_mode(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        let path = self.through_links(&self.resolve(path.to_bytes()), true)?;
        let (_, status) = self.nodes.get_mut(&path).ok_or(Errno::ENOENT)?;
        status.mode = (status.mode & FILE_TYPE) | (mode & 0o7777);
        Ok(())
    }

    fn make_hard_link(&mut self, existing: &CStr, path: &CStr) -> Result<(), Errno> {
        let existing = self.through_links(&self.resolve(existing.to_bytes()), false)?;
        let path = self.through_links(&self.resolve(path.to_bytes()), false)?;
        let (node, status) = self.nodes.get(&existing).cloned().ok_or(Errno::ENOENT)?;
        if matches!(node, Node::Directory) {
            return Err(Errno::EPERM);
        }
        if self.nodes.contains_key(&path) {
            return Err(Errno::EEXIST);
        }
        self.parent_directory(&path)?;
        if self
            .cross_device
            .iter()
            .any(|device| path.starts_with(device) != existing.starts_with(device))
        {
            return Err(Errno::EXDEV);
        }
        let linked = Status {
            links: status.links + 1,
            ..status
        };
        self.nodes.insert(existing, (node.clone(), linked));
        self.nodes.insert(path, (node, linked));
        Ok(())
    }

    fn now(&mut self) -> u64 {
        self.now.max(0) as u64
    }

    fn now_with_nanoseconds(&mut self) -> Duration {
        Duration {
            seconds: self.now(),
            nanoseconds: self.now_nanoseconds,
        }
    }

    fn set_time(&mut self, since_epoch: Duration) -> Result<(), Errno> {
        if let Some(errno) = self.set_time_error {
            return Err(errno);
        }
        self.set_times.push(since_epoch);
        self.now = since_epoch.seconds as i64;
        self.now_nanoseconds = since_epoch.nanoseconds;
        Ok(())
    }

    fn working_directory(&mut self, out: &mut [u8]) -> Result<usize, Errno> {
        let directory = self.working_directory.as_bytes();
        let held = out.get_mut(..directory.len()).ok_or(Errno(34))?;
        held.copy_from_slice(directory);
        Ok(directory.len())
    }

    fn device_control(
        &mut self,
        descriptor: usize,
        request: u32,
        argument: &mut [u8],
    ) -> Result<(), Errno> {
        if self.file(descriptor)?.path != crate::wm::DEVICE.to_bytes() {
            return Err(Errno::ENOTTY);
        }
        let word = |bytes: &[u8], at: usize| {
            u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        match (request, argument.len()) {
            (crate::wm::LIST_WINDOW, crate::wm::LIST_SIZE) => {
                let window = self
                    .windows
                    .get(word(argument, 0) as usize)
                    .ok_or(Errno::ENOENT)?;
                let [x, y, width, height] = window.rect;
                let words = [
                    window.id,
                    window.state,
                    x,
                    y,
                    width,
                    height,
                    window.client,
                    window.title.len() as u32,
                ];
                for (index, value) in words.into_iter().enumerate() {
                    argument[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
                }
                argument[32..32 + window.title.len()].copy_from_slice(&window.title);
                Ok(())
            }
            (crate::wm::MANAGE_WINDOW, crate::wm::MANAGE_SIZE) => {
                let request = [0, 4, 8, 12].map(|at| word(argument, at));
                if !self.windows.iter().any(|window| window.id == request[0]) {
                    return Err(Errno::EINVAL);
                }
                self.managed.push(request);
                Ok(())
            }
            (crate::wm::SEND_INPUT, crate::wm::SEND_INPUT_SIZE) => {
                let request = [0, 4, 8, 12, 16, 20].map(|at| word(argument, at));
                let Some(window) = self.windows.iter().find(|window| window.id == request[0])
                else {
                    return Err(Errno::ENOENT);
                };
                let pointer = matches!(request[1], 0 | 1 | 5 | 6);
                let sideways = request[1] == 2 && request[4] != 0;
                if window.client == crate::wm::KERNEL_CLIENT && (pointer || sideways) {
                    return Err(Errno::EOPNOTSUPP);
                }
                if !self.input_results.is_empty()
                    && let Some(errno) = self.input_results.remove(0)
                {
                    return Err(errno);
                }
                let dialog = self
                    .dialogs
                    .iter()
                    .find(|(parent, _)| *parent == request[0])
                    .map(|&(_, dialog)| dialog);
                let (receiver, outcome) = match dialog {
                    Some(dialog) if pointer || request[1] == 2 => (dialog, 2),
                    Some(dialog) => (dialog, 1),
                    None => (request[0], 0),
                };
                if outcome != 2 {
                    self.sent_input.push(request);
                }
                argument[0..4].copy_from_slice(&receiver.to_ne_bytes());
                argument[4..8].copy_from_slice(&outcome.to_ne_bytes());
                Ok(())
            }
            (crate::wm::GET_MENU, crate::wm::GET_MENU_SIZE) => {
                let window = word(argument, 0);
                if !self.windows.iter().any(|known| known.id == window) {
                    return Err(Errno::ENOENT);
                }
                let pool = self.menus.get(&window).map_or(&[][..], Vec::as_slice);
                argument[4..8].copy_from_slice(&(pool.len() as u32).to_ne_bytes());
                argument[8..8 + pool.len()].copy_from_slice(pool);
                Ok(())
            }
            (crate::wm::CHOOSE_MENU, crate::wm::CHOOSE_MENU_SIZE) => {
                let request = [0, 4, 8].map(|at| word(argument, at));
                if !self.windows.iter().any(|known| known.id == request[0]) {
                    return Err(Errno::ENOENT);
                }
                if !self.input_results.is_empty()
                    && let Some(errno) = self.input_results.remove(0)
                {
                    return Err(errno);
                }
                self.chosen_menus.push(request);
                let length = word(argument, 12) as usize;
                self.chosen_labels.push(argument[16..16 + length].to_vec());
                Ok(())
            }
            (crate::wm::ACT_ON_ELEMENT, crate::wm::ACT_ON_ELEMENT_SIZE) => {
                let request = [0, 4, 8, 12].map(|at| word(argument, at));
                if !self.windows.iter().any(|known| known.id == request[0]) {
                    return Err(Errno::ENOENT);
                }
                if !self.trees.contains_key(&request[0]) {
                    return Err(Errno::ETIMEDOUT);
                }
                if !self.input_results.is_empty()
                    && let Some(errno) = self.input_results.remove(0)
                {
                    return Err(errno);
                }
                let text = argument[24..24 + request[3] as usize].to_vec();
                self.acted.push((request[0], request[1], request[2], text));
                argument[16..20].copy_from_slice(&self.act_outcome.to_ne_bytes());
                Ok(())
            }
            _ => Err(Errno::ENOTTY),
        }
    }

    fn capture_window_rows(
        &mut self,
        descriptor: usize,
        window: u32,
        first_row: u32,
        row_count: u32,
        pixels: &mut [u8],
    ) -> Result<CapturedRows, Errno> {
        if self.file(descriptor)?.path != crate::wm::DEVICE.to_bytes() {
            return Err(Errno::ENOTTY);
        }
        let call = self.capture_calls;
        self.capture_calls += 1;
        if let Some((at, errno)) = self.capture_fails_at
            && at == call
        {
            return Err(errno);
        }
        if let Some((at, width, height)) = self.resized_at
            && at == call
        {
            let pixels = (0..width * height).map(|index| index * 0x0101).collect();
            self.window_pixels.insert(window, (width, height, pixels));
        }
        let Some((width, height, content)) = self.window_pixels.get(&window) else {
            return match self.windows.iter().find(|known| known.id == window) {
                Some(known) if known.client == crate::wm::KERNEL_CLIENT => Err(Errno::EOPNOTSUPP),
                _ => Err(Errno::ENOENT),
            };
        };
        let (width, height) = (*width, *height);
        let rows = height
            .checked_sub(first_row)
            .ok_or(Errno::EINVAL)?
            .min(row_count);
        let bytes = rows as usize * width as usize * 4;
        if bytes > pixels.len() {
            return Err(Errno::EINVAL);
        }
        let start = first_row as usize * width as usize;
        for (pixel, place) in content[start..start + rows as usize * width as usize]
            .iter()
            .zip(pixels.as_chunks_mut::<4>().0.iter_mut())
        {
            place.copy_from_slice(&pixel.to_ne_bytes());
        }
        Ok(CapturedRows {
            width,
            height,
            rows,
        })
    }

    fn window_tree(
        &mut self,
        descriptor: usize,
        window: u32,
        tree: &mut [u8],
    ) -> Result<WindowTree, Errno> {
        if self.file(descriptor)?.path != crate::wm::DEVICE.to_bytes() {
            return Err(Errno::ENOTTY);
        }
        if !self.windows.iter().any(|known| known.id == window) {
            return Err(Errno::ENOENT);
        }
        let Some(bytes) = self.trees.get(&window) else {
            return Ok(WindowTree {
                pixels_only: true,
                length: 0,
            });
        };
        tree.get_mut(..bytes.len())
            .ok_or(Errno::EINVAL)?
            .copy_from_slice(bytes);
        Ok(WindowTree {
            pixels_only: false,
            length: bytes.len(),
        })
    }

    fn open_datagram_socket(&mut self, kind: DatagramKind) -> Result<usize, Errno> {
        let index = self.network.sockets.len();
        self.network.sockets.push(FakeSocket {
            kind,
            port: FIRST_SOCKET_PORT + index as u16,
            queue: VecDeque::new(),
            open: true,
        });
        Ok(FIRST_SOCKET_DESCRIPTOR + index)
    }

    fn send_to(
        &mut self,
        descriptor: usize,
        bytes: &[u8],
        address: Ipv4Address,
        port: u16,
    ) -> Result<usize, Errno> {
        let kind = self.network.socket(descriptor)?.kind;
        if let Some(errno) = self.network.send_error {
            return Err(errno);
        }
        self.network
            .sent
            .push((kind, address, port, bytes.to_vec()));
        self.network.answer(descriptor, bytes, address, port);
        Ok(bytes.len())
    }

    fn receive_from(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<(usize, Sender)>, Errno> {
        let round_trip = self.network.round_trip_us;
        let Some((bytes, sender)) = self.network.socket(descriptor)?.queue.pop_front() else {
            self.network.monotonic_us += u64::from(timeout_ms) * 1000;
            return Ok(None);
        };
        self.network.monotonic_us += round_trip;
        let count = bytes.len().min(buffer.len());
        buffer[..count].copy_from_slice(&bytes[..count]);
        Ok(Some((count, sender)))
    }

    fn monotonic_us(&mut self) -> u64 {
        self.network.monotonic_us
    }

    fn open_stream_socket(&mut self) -> Result<usize, Errno> {
        self.network.streams.push(FakeStream {
            open: true,
            ..FakeStream::default()
        });
        Ok(FIRST_STREAM_DESCRIPTOR + self.network.streams.len() - 1)
    }

    fn connect(
        &mut self,
        descriptor: usize,
        address: Ipv4Address,
        port: u16,
        timeout_ms: u32,
    ) -> Result<(), Errno> {
        let served = self
            .network
            .pages
            .iter()
            .any(|page| (page.address, page.port) == (address, port))
            || self
                .network
                .raw_answers
                .iter()
                .any(|(at, _)| *at == (address, port));
        let reachable = self.network.reachable.contains(&address);
        let stream = self.network.stream(descriptor)?;
        match (served, reachable) {
            (true, _) => {
                stream.address = Some((address, port));
                Ok(())
            }
            (false, true) => Err(Errno::ECONNREFUSED),
            (false, false) => {
                self.network.monotonic_us += u64::from(timeout_ms) * 1000;
                Err(Errno::ETIMEDOUT)
            }
        }
    }

    fn send(&mut self, descriptor: usize, bytes: &[u8]) -> Result<(), Errno> {
        let stream = self.network.stream(descriptor)?;
        let address = stream.address.ok_or(Errno::EPIPE)?;
        stream.request.extend_from_slice(bytes);
        let answered = stream.answered;
        let raw = self
            .network
            .raw_answers
            .iter()
            .find(|(at, _)| *at == address)
            .map(|(_, answer)| answer.clone());
        if let (false, Some(raw)) = (answered, raw) {
            let stream = self.network.stream(descriptor)?;
            stream.response.extend(raw);
            stream.answered = true;
            return Ok(());
        }
        let stream = self.network.stream(descriptor)?;
        if stream.answered || !stream.request.windows(4).any(|end| end == b"\r\n\r\n") {
            return Ok(());
        }
        let request = stream.request.clone();
        let response = self.network.page(address, &request);
        let stream = self.network.stream(descriptor)?;
        stream.response.extend(response);
        stream.answered = true;
        self.network.requests.push(request);
        Ok(())
    }

    fn receive(
        &mut self,
        descriptor: usize,
        buffer: &mut [u8],
        timeout_ms: u32,
    ) -> Result<Option<usize>, Errno> {
        let stream = self.network.stream(descriptor)?;
        if stream.response.is_empty() {
            if stream.answered {
                return Ok(Some(0));
            }
            self.network.monotonic_us += u64::from(timeout_ms) * 1000;
            return Ok(None);
        }
        let count = stream.response.len().min(buffer.len()).min(STREAM_PIECE);
        for (slot, byte) in buffer.iter_mut().zip(stream.response.drain(..count)) {
            *slot = byte;
        }
        Ok(Some(count))
    }

    fn random(&mut self, out: &mut [u8]) -> Result<(), Errno> {
        for byte in out.iter_mut() {
            self.network.random_counter = self.network.random_counter.wrapping_add(1);
            *byte = self.network.random_counter;
        }
        Ok(())
    }

    fn signal_init(&mut self, signal: u32) -> Result<(), Errno> {
        self.init_signals.push(signal);
        self.init_signal_error.map_or(Ok(()), Err)
    }

    fn terminal_columns(&mut self, descriptor: usize) -> Option<u16> {
        self.terminal_width
            .filter(|_| descriptor == STANDARD_OUTPUT)
    }
}

impl Fake {
    fn resolve(&self, path: &[u8]) -> Vec<u8> {
        if path.first() == Some(&b'/') || self.working_directory == "/" {
            return path.to_vec();
        }
        let joined = [self.working_directory.as_bytes(), b"/", path].concat();
        eggos_files::names::normalized(&joined).map_or(joined, |path| path.as_bytes().to_vec())
    }

    fn parent_directory(&self, path: &[u8]) -> Result<(), Errno> {
        let (directory, _) = split_last_component(path);
        match self.nodes.get(directory) {
            Some((Node::Directory, _)) => Ok(()),
            Some(_) => Err(Errno::ENOTDIR),
            None if directory == b"." => Ok(()),
            None => Err(Errno::ENOENT),
        }
    }
}
