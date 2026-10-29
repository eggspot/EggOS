use crate::files::{Files, Handle, Kind, Status};
use eggos_runtime::errno::Errno;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    File(Vec<u8>, u32),
    Directory,
    Link(Vec<u8>),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Create(String),
    MakeDirectory(String),
    MakeLink(String, String),
    RemoveFile(String),
    RemoveDirectory(String),
    Rename(String, String),
    Sync,
}

#[derive(Debug, Default)]
pub struct FakeFiles {
    pub nodes: Vec<(Vec<u8>, Node)>,
    pub read_only: Vec<Vec<u8>>,
    pub space: Option<usize>,
    pub log: Vec<Operation>,
    open: Vec<Option<(Vec<u8>, usize, bool)>>,
}

const MAX_LINK_HOPS: usize = 40;

fn text(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => b"/",
        Some(slash) => &path[..slash],
        None => b"",
    }
}

impl FakeFiles {
    pub fn new() -> Self {
        let mut files = Self::default();
        files.directory("/");
        files.directory("/Applications");
        files.directory("/media");
        files.directory("/media/host");
        files.directory("/usr");
        files.directory("/usr/lib");
        files.directory("/usr/lib/eggos");
        files.file("/usr/lib/eggos/windows", b"\x7fELF layer");
        files.file("/usr/lib/eggos/macos", b"\x7fELF layer");
        files
    }

    pub fn directory(&mut self, path: &str) {
        self.nodes.push((path.as_bytes().to_vec(), Node::Directory));
    }

    pub fn file(&mut self, path: &str, bytes: &[u8]) {
        self.nodes
            .push((path.as_bytes().to_vec(), Node::File(bytes.to_vec(), 0o755)));
    }

    pub fn other(&mut self, path: &str) {
        self.nodes.push((path.as_bytes().to_vec(), Node::Other));
    }

    pub fn link(&mut self, path: &str, target: &str) {
        self.nodes.push((
            path.as_bytes().to_vec(),
            Node::Link(target.as_bytes().to_vec()),
        ));
    }

    fn resolved(&self, path: &[u8]) -> Result<Vec<u8>, Errno> {
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
            let mut next = done.clone();
            next.push(b'/');
            next.extend_from_slice(&part);
            match self.node(&next) {
                Some(Node::Link(target)) => {
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
                Some(_) => done = next,
                None if rest.is_empty() => done = next,
                None => return Err(Errno::ENOENT),
            }
        }
        match done.is_empty() {
            true => Ok(b"/".to_vec()),
            false => Ok(done),
        }
    }

    pub fn find(&self, path: &str) -> Option<&Node> {
        self.node(path.as_bytes())
    }

    pub fn paths_under(&self, prefix: &str) -> Vec<String> {
        self.nodes
            .iter()
            .map(|(path, _)| text(path))
            .filter(|path| path.starts_with(prefix))
            .collect()
    }

    fn node(&self, path: &[u8]) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|(at, _)| at == path)
            .map(|(_, node)| node)
    }

    fn position(&self, path: &[u8]) -> Option<usize> {
        self.nodes.iter().position(|(at, _)| at == path)
    }

    fn writable(&self, path: &[u8]) -> Result<(), Errno> {
        match self.read_only.iter().any(|prefix| path.starts_with(prefix)) {
            true => Err(Errno::EROFS),
            false => Ok(()),
        }
    }

    fn used(&self) -> usize {
        self.nodes
            .iter()
            .map(|(_, node)| match node {
                Node::File(bytes, _) => bytes.len(),
                _ => 0,
            })
            .sum()
    }

    fn new_node(&mut self, path: &[u8], node: Node) -> Result<(), Errno> {
        self.writable(path)?;
        if self.node(path).is_some() {
            return Err(Errno::EEXIST);
        }
        match self.node(parent(path)) {
            Some(Node::Directory) => {}
            Some(_) => return Err(Errno::ENOTDIR),
            None => return Err(Errno::ENOENT),
        }
        self.nodes.push((path.to_vec(), node));
        Ok(())
    }
}

impl Files for FakeFiles {
    fn status(&mut self, path: &[u8]) -> Result<Status, Errno> {
        match self.node(path).ok_or(Errno::ENOENT)? {
            Node::File(bytes, permissions) => Ok(Status {
                kind: Kind::File,
                size: bytes.len() as u64,
                permissions: *permissions,
                modified: 0,
                device: 0,
            }),
            Node::Directory => Ok(Status {
                kind: Kind::Directory,
                size: 0,
                permissions: 0o755,
                modified: 0,
                device: 0,
            }),
            Node::Link(target) => Ok(Status {
                kind: Kind::Link,
                size: target.len() as u64,
                permissions: 0o777,
                modified: 0,
                device: 0,
            }),
            Node::Other => Ok(Status {
                kind: Kind::Other,
                size: 0,
                permissions: 0o777,
                modified: 0,
                device: 0,
            }),
        }
    }

    fn status_following(&mut self, path: &[u8]) -> Result<Status, Errno> {
        let resolved = self.resolved(path)?;
        self.status(&resolved)
    }

    fn read_link(&mut self, path: &[u8], target: &mut [u8]) -> Result<usize, Errno> {
        match self.node(path).ok_or(Errno::ENOENT)? {
            Node::Link(text) => {
                let place = target.get_mut(..text.len()).ok_or(Errno::ENAMETOOLONG)?;
                place.copy_from_slice(text);
                Ok(text.len())
            }
            _ => Err(Errno::EINVAL),
        }
    }

    fn make_link(&mut self, target: &[u8], path: &[u8]) -> Result<(), Errno> {
        if target.is_empty() {
            return Err(Errno::ENOENT);
        }
        self.new_node(path, Node::Link(target.to_vec()))?;
        let made = Operation::MakeLink(text(target), text(path));
        self.log.push(made);
        Ok(())
    }

    fn entry(
        &mut self,
        directory: &[u8],
        index: usize,
        name: &mut [u8; 256],
    ) -> Result<Option<usize>, Errno> {
        let directory = self.resolved(directory)?;
        match self.node(&directory).ok_or(Errno::ENOENT)? {
            Node::Directory => {}
            _ => return Err(Errno::ENOTDIR),
        }
        let child = self
            .nodes
            .iter()
            .filter(|(path, _)| path.as_slice() != b"/" && parent(path) == directory.as_slice())
            .nth(index);
        Ok(child.map(|(path, _)| {
            let start = path.iter().rposition(|&byte| byte == b'/').unwrap_or(0) + 1;
            let file = &path[start..];
            name[..file.len()].copy_from_slice(file);
            file.len()
        }))
    }

    fn open(&mut self, path: &[u8]) -> Result<Handle, Errno> {
        let path = self.resolved(path)?;
        match self.node(&path).ok_or(Errno::ENOENT)? {
            Node::File(..) => {}
            _ => return Err(Errno::EISDIR),
        }
        self.open.push(Some((path, 0, false)));
        Ok(self.open.len() - 1)
    }

    fn create(&mut self, path: &[u8], permissions: u32) -> Result<Handle, Errno> {
        self.new_node(path, Node::File(Vec::new(), permissions))?;
        self.log.push(Operation::Create(text(path)));
        self.open.push(Some((path.to_vec(), 0, true)));
        Ok(self.open.len() - 1)
    }

    fn read(&mut self, file: Handle, buffer: &mut [u8]) -> Result<usize, Errno> {
        let Some(Some((path, offset, false))) = self.open.get(file).cloned() else {
            return Err(Errno(9));
        };
        let Some(Node::File(bytes, _)) = self.node(&path) else {
            return Err(Errno::EIO);
        };
        let count = bytes.len().saturating_sub(offset).min(buffer.len());
        buffer[..count].copy_from_slice(&bytes[offset..offset + count]);
        self.open[file] = Some((path, offset + count, false));
        Ok(count)
    }

    fn read_at(&mut self, file: Handle, offset: u64, buffer: &mut [u8]) -> Result<usize, Errno> {
        let Some(Some((path, _, false))) = self.open.get(file).cloned() else {
            return Err(Errno(9));
        };
        let Some(Node::File(bytes, _)) = self.node(&path) else {
            return Err(Errno::EIO);
        };
        let start = (offset as usize).min(bytes.len());
        let count = (bytes.len() - start).min(buffer.len());
        buffer[..count].copy_from_slice(&bytes[start..start + count]);
        Ok(count)
    }

    fn write(&mut self, file: Handle, bytes: &[u8]) -> Result<(), Errno> {
        let Some(Some((path, _, true))) = self.open.get(file).cloned() else {
            return Err(Errno(9));
        };
        if self
            .space
            .is_some_and(|space| self.used() + bytes.len() > space)
        {
            return Err(Errno::ENOSPC);
        }
        let at = self.position(&path).ok_or(Errno::EIO)?;
        if let Node::File(contents, _) = &mut self.nodes[at].1 {
            contents.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn sync_file(&mut self, file: Handle) -> Result<(), Errno> {
        match self.open.get(file) {
            Some(Some(_)) => Ok(()),
            _ => Err(Errno(9)),
        }
    }

    fn close(&mut self, file: Handle) {
        assert!(
            self.open.get(file).is_some_and(Option::is_some),
            "closed twice"
        );
        self.open[file] = None;
    }

    fn make_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.new_node(path, Node::Directory)?;
        self.log.push(Operation::MakeDirectory(text(path)));
        Ok(())
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.writable(path)?;
        let at = self.position(path).ok_or(Errno::ENOENT)?;
        if self.nodes[at].1 == Node::Directory {
            return Err(Errno::EISDIR);
        }
        self.nodes.remove(at);
        self.log.push(Operation::RemoveFile(text(path)));
        Ok(())
    }

    fn remove_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.writable(path)?;
        let at = self.position(path).ok_or(Errno::ENOENT)?;
        if self.nodes[at].1 != Node::Directory {
            return Err(Errno::ENOTDIR);
        }
        if self
            .nodes
            .iter()
            .any(|(other, _)| parent(other) == path && other.as_slice() != b"/")
        {
            return Err(Errno::ENOTEMPTY);
        }
        self.nodes.remove(at);
        self.log.push(Operation::RemoveDirectory(text(path)));
        Ok(())
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Errno> {
        self.writable(to)?;
        let at = self.position(from).ok_or(Errno::ENOENT)?;
        if let Some(existing) = self.position(to) {
            self.nodes.remove(existing);
        }
        let at = self.position(from).unwrap_or(at);
        self.nodes[at].0 = to.to_vec();
        self.log.push(Operation::Rename(text(from), text(to)));
        Ok(())
    }

    fn sync(&mut self) {
        self.log.push(Operation::Sync);
    }
}

impl FakeFiles {
    pub fn open_count(&self) -> usize {
        self.open.iter().filter(|open| open.is_some()).count()
    }
}
