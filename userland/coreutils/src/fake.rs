use crate::system::{
    DIRECTORY, Duration, STANDARD_ERROR, STANDARD_INPUT, STANDARD_OUTPUT, Status, SymbolicLinks,
    System, SystemName, UTSNAME_FIELD, UTSNAME_SIZE,
};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use std::collections::BTreeMap;

const REGULAR_FILE: u32 = 0o100_000;
const FIRST_FILE_DESCRIPTOR: usize = 3;

#[derive(Debug, Clone)]
pub enum Node {
    Directory,
    File(Vec<u8>),
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
    pub reboot_error: Option<Errno>,
    open: Vec<Option<OpenFile>>,
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
            reboot_error: Some(Errno::EPERM),
            open: Vec::new(),
        }
    }
}

impl Fake {
    pub fn with_directory(mut self, path: &str, modified: i64) -> Self {
        let status = Status {
            mode: DIRECTORY | 0o755,
            links: 2,
            size: 0,
            blocks: 0,
            modified,
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
            ..Status::default()
        };
        self.nodes.insert(
            path.as_bytes().to_vec(),
            (Node::File(content.to_vec()), status),
        );
        self
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
            _ => Err(Errno(9)),
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
        let path = path.to_bytes().to_vec();
        if !self.nodes.contains_key(&path) {
            return Err(Errno::ENOENT);
        }
        self.open.push(Some(OpenFile { path, position: 0 }));
        Ok(self.open.len() - 1 + FIRST_FILE_DESCRIPTOR)
    }

    fn close(&mut self, descriptor: usize) {
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
        _: SymbolicLinks,
    ) -> Result<Status, Errno> {
        let path = match directory {
            Some(descriptor) => {
                let directory = self.file(descriptor)?.path.clone();
                match directory.as_slice() {
                    b"/" => [b"/", path.to_bytes()].concat(),
                    _ => [&directory[..], b"/", path.to_bytes()].concat(),
                }
            }
            None => path.to_bytes().to_vec(),
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
        Ok(())
    }

    fn reboot(&mut self, command: u32) -> Result<(), Errno> {
        self.rebooted.push(command);
        self.reboot_error.map_or(Ok(()), Err)
    }
}
