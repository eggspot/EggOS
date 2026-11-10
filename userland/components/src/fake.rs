use eggos_app::files::{Files, Handle, Kind, Space, Status};
use eggos_runtime::errno::Errno;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Directory,
    File(Vec<u8>, u32),
    Link(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct FakeFiles {
    pub nodes: BTreeMap<Vec<u8>, Node>,
    open: Vec<Option<(Vec<u8>, u64)>>,
    pub available: Option<u64>,
    pub read_only: Vec<Vec<u8>>,
    pub fail_reads_of: Option<Vec<u8>>,
    pub syncs: usize,
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => b"/",
        Some(at) => &path[..at],
        None => b"",
    }
}

impl FakeFiles {
    pub fn new() -> Self {
        let mut files = Self::default();
        files.nodes.insert(b"/".to_vec(), Node::Directory);
        files
    }

    pub fn folder(&mut self, path: &str) -> &mut Self {
        let mut at = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            at.push('/');
            at.push_str(part);
            self.nodes
                .entry(at.as_bytes().to_vec())
                .or_insert(Node::Directory);
        }
        self
    }

    pub fn file(&mut self, path: &str, bytes: &[u8]) -> &mut Self {
        self.folder(core::str::from_utf8(parent(path.as_bytes())).unwrap());
        self.nodes
            .insert(path.as_bytes().to_vec(), Node::File(bytes.to_vec(), 0o644));
        self
    }

    pub fn link(&mut self, path: &str, target: &str) -> &mut Self {
        self.folder(core::str::from_utf8(parent(path.as_bytes())).unwrap());
        self.nodes.insert(
            path.as_bytes().to_vec(),
            Node::Link(target.as_bytes().to_vec()),
        );
        self
    }

    pub fn contents(&self, path: &str) -> Option<&[u8]> {
        match self.nodes.get(path.as_bytes()) {
            Some(Node::File(bytes, _)) => Some(bytes),
            _ => None,
        }
    }

    pub fn paths_under(&self, prefix: &str) -> Vec<String> {
        self.nodes
            .keys()
            .filter(|path| {
                path.starts_with(prefix.as_bytes()) && path.as_slice() != prefix.as_bytes()
            })
            .map(|path| String::from_utf8(path.clone()).unwrap())
            .collect()
    }

    fn writable(&self, path: &[u8]) -> Result<(), Errno> {
        match self.read_only.iter().any(|prefix| path.starts_with(prefix)) {
            true => Err(Errno::EACCES),
            false => Ok(()),
        }
    }

    fn resolve(&self, path: &[u8]) -> Result<Vec<u8>, Errno> {
        let mut path = path.to_vec();
        for _ in 0..8 {
            match self.nodes.get(&path) {
                Some(Node::Link(target)) => path = target.clone(),
                Some(_) => return Ok(path),
                None => return Err(Errno::ENOENT),
            }
        }
        Err(Errno::ELOOP)
    }

    fn status_of(&self, path: &[u8]) -> Result<Status, Errno> {
        let (kind, size, permissions) = match self.nodes.get(path).ok_or(Errno::ENOENT)? {
            Node::Directory => (Kind::Directory, 0, 0o755),
            Node::File(bytes, mode) => (Kind::File, bytes.len() as u64, *mode),
            Node::Link(target) => (Kind::Link, target.len() as u64, 0o777),
        };
        Ok(Status {
            kind,
            size,
            permissions,
            modified: 0,
            device: 0,
        })
    }

    fn children(&self, directory: &[u8]) -> Vec<Vec<u8>> {
        self.nodes
            .keys()
            .filter(|path| path.as_slice() != b"/" && parent(path) == directory)
            .map(|path| {
                let at = path.iter().rposition(|&byte| byte == b'/').unwrap();
                path[at + 1..].to_vec()
            })
            .collect()
    }
}

impl Files for FakeFiles {
    fn status(&mut self, path: &[u8]) -> Result<Status, Errno> {
        self.status_of(path)
    }

    fn status_following(&mut self, path: &[u8]) -> Result<Status, Errno> {
        let path = self.resolve(path)?;
        self.status_of(&path)
    }

    fn read_link(&mut self, path: &[u8], target: &mut [u8]) -> Result<usize, Errno> {
        match self.nodes.get(path) {
            Some(Node::Link(link)) => {
                target[..link.len()].copy_from_slice(link);
                Ok(link.len())
            }
            Some(_) => Err(Errno::EINVAL),
            None => Err(Errno::ENOENT),
        }
    }

    fn make_link(&mut self, target: &[u8], path: &[u8]) -> Result<(), Errno> {
        if self.nodes.contains_key(path) {
            return Err(Errno::EEXIST);
        }
        self.nodes
            .insert(path.to_vec(), Node::Link(target.to_vec()));
        Ok(())
    }

    fn entry(
        &mut self,
        directory: &[u8],
        index: usize,
        name: &mut [u8; 256],
    ) -> Result<Option<usize>, Errno> {
        if self.nodes.get(directory) != Some(&Node::Directory) {
            return Err(Errno::ENOENT);
        }
        Ok(self.children(directory).get(index).map(|child| {
            name[..child.len()].copy_from_slice(child);
            child.len()
        }))
    }

    fn open(&mut self, path: &[u8]) -> Result<Handle, Errno> {
        let path = self.resolve(path)?;
        if !matches!(self.nodes.get(&path), Some(Node::File(..))) {
            return Err(Errno::EISDIR);
        }
        self.open.push(Some((path, 0)));
        Ok(self.open.len() - 1)
    }

    fn create(&mut self, path: &[u8], permissions: u32) -> Result<Handle, Errno> {
        if self.nodes.contains_key(path) {
            return Err(Errno::EEXIST);
        }
        if self.nodes.get(parent(path)) != Some(&Node::Directory) {
            return Err(Errno::ENOENT);
        }
        self.writable(path)?;
        self.nodes
            .insert(path.to_vec(), Node::File(Vec::new(), permissions));
        self.open.push(Some((path.to_vec(), 0)));
        Ok(self.open.len() - 1)
    }

    fn read(&mut self, file: Handle, buffer: &mut [u8]) -> Result<usize, Errno> {
        let (path, position) = self.open[file].clone().ok_or(Errno::EINVAL)?;
        let read = self.read_at(file, position, buffer)?;
        self.open[file] = Some((path, position + read as u64));
        Ok(read)
    }

    fn read_at(&mut self, file: Handle, offset: u64, buffer: &mut [u8]) -> Result<usize, Errno> {
        let (path, _) = self.open[file].clone().ok_or(Errno::EINVAL)?;
        if self.fail_reads_of.as_deref() == Some(path.as_slice()) && offset > 0 {
            return Err(Errno::EIO);
        }
        let Some(Node::File(bytes, _)) = self.nodes.get(&path) else {
            return Err(Errno::ENOENT);
        };
        let start = (offset as usize).min(bytes.len());
        let length = buffer.len().min(bytes.len() - start);
        buffer[..length].copy_from_slice(&bytes[start..start + length]);
        Ok(length)
    }

    fn write(&mut self, file: Handle, bytes: &[u8]) -> Result<(), Errno> {
        let (path, _) = self.open[file].clone().ok_or(Errno::EINVAL)?;
        if let Some(available) = self.available.as_mut() {
            if (bytes.len() as u64) > *available {
                return Err(Errno::ENOSPC);
            }
            *available -= bytes.len() as u64;
        }
        match self.nodes.get_mut(&path) {
            Some(Node::File(contents, _)) => {
                contents.extend_from_slice(bytes);
                Ok(())
            }
            _ => Err(Errno::ENOENT),
        }
    }

    fn sync_file(&mut self, _file: Handle) -> Result<(), Errno> {
        Ok(())
    }

    fn close(&mut self, file: Handle) {
        self.open[file] = None;
    }

    fn make_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        if self.nodes.contains_key(path) {
            return Err(Errno::EEXIST);
        }
        if self.nodes.get(parent(path)) != Some(&Node::Directory) {
            return Err(Errno::ENOENT);
        }
        self.writable(path)?;
        self.nodes.insert(path.to_vec(), Node::Directory);
        Ok(())
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.writable(path)?;
        match self.nodes.get(path) {
            Some(Node::Directory) => Err(Errno::EISDIR),
            Some(_) => {
                self.nodes.remove(path);
                Ok(())
            }
            None => Err(Errno::ENOENT),
        }
    }

    fn remove_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        if !self.children(path).is_empty() {
            return Err(Errno::ENOTEMPTY);
        }
        match self.nodes.remove(path) {
            Some(_) => Ok(()),
            None => Err(Errno::ENOENT),
        }
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Errno> {
        if !self.nodes.contains_key(from) {
            return Err(Errno::ENOENT);
        }
        if self.nodes.contains_key(to) {
            return Err(Errno::EEXIST);
        }
        let moved: Vec<Vec<u8>> = self
            .nodes
            .keys()
            .filter(|path| {
                path.as_slice() == from
                    || (path.starts_with(from) && path.get(from.len()) == Some(&b'/'))
            })
            .cloned()
            .collect();
        for path in moved {
            let node = self.nodes.remove(&path).unwrap();
            let mut renamed = to.to_vec();
            renamed.extend_from_slice(&path[from.len()..]);
            self.nodes.insert(renamed, node);
        }
        Ok(())
    }

    fn sync(&mut self) {
        self.syncs += 1;
    }

    fn space(&mut self, _path: &[u8]) -> Result<Space, Errno> {
        match self.available {
            Some(available) => Ok(Space {
                total: available,
                available,
                read_only: false,
            }),
            None => Err(Errno::ENOSYS),
        }
    }
}
