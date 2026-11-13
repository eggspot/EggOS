//! `/sys` (D558): the few files programs read to size themselves and that make their
//! allocators fall back from guessing: `devices/system/cpu/{online,possible,present}` (the CPUs as
//! Linux's ranges, `0-3`), `kernel/mm/transparent_hugepage/enabled` (`always madvise [never]`:
//! EggOS gives programs no transparent huge pages) and `defrag`. Read-only; each file's text is
//! made when it is opened.

use super::{Attributes, DirectoryEntry, FileSystem, Name, NodeKind, Usage};
use alloc::vec::Vec;

const ROOT: u64 = 1;
const DEVICES: u64 = 2;
const SYSTEM: u64 = 3;
const CPU: u64 = 4;
const ONLINE: u64 = 5;
const POSSIBLE: u64 = 6;
const PRESENT: u64 = 7;
const KERNEL: u64 = 8;
const MM: u64 = 9;
const TRANSPARENT_HUGEPAGE: u64 = 10;
const ENABLED: u64 = 11;
const DEFRAG: u64 = 12;

/// Each node with its name and its parent.
const NODES: [(u64, &[u8], u64); 11] = [
    (DEVICES, b"devices", ROOT),
    (SYSTEM, b"system", DEVICES),
    (CPU, b"cpu", SYSTEM),
    (ONLINE, b"online", CPU),
    (POSSIBLE, b"possible", CPU),
    (PRESENT, b"present", CPU),
    (KERNEL, b"kernel", ROOT),
    (MM, b"mm", KERNEL),
    (TRANSPARENT_HUGEPAGE, b"transparent_hugepage", MM),
    (ENABLED, b"enabled", TRANSPARENT_HUGEPAGE),
    (DEFRAG, b"defrag", TRANSPARENT_HUGEPAGE),
];

/// The device number `stat` reports for its files.
const DEVICE: (u32, u32) = (0, 21);

/// `/sys`, mounted at boot.
#[derive(Debug)]
pub struct SystemFiles;

fn is_directory(node: u64) -> bool {
    matches!(node, ROOT | DEVICES | SYSTEM | CPU | KERNEL | MM | TRANSPARENT_HUGEPAGE)
}

/// The CPUs `0..count` as Linux writes a CPU list: `0`, or `0-<last>`.
fn cpu_range(count: usize) -> Vec<u8> {
    let text = match count {
        0 | 1 => alloc::string::String::from("0\n"),
        many => alloc::format!("0-{}\n", many - 1),
    };
    text.into_bytes()
}

impl FileSystem for SystemFiles {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn usage(&self) -> Usage {
        Usage::without_blocks(super::magic::SYSFS, true)
    }

    fn kind(&self, node: u64) -> NodeKind {
        match is_directory(node) {
            true => NodeKind::Directory,
            false => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let kind = self.kind(node);
        Attributes {
            kind,
            permissions: if kind == NodeKind::Directory { 0o555 } else { 0o444 },
            file_id: node,
            links: if kind == NodeKind::Directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size: 4096,
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        NODES
            .iter()
            .find(|&&(_, file, parent)| parent == directory && file == name)
            .map(|&(node, _, _)| node)
    }

    fn parent(&self, node: u64) -> u64 {
        NODES
            .iter()
            .find(|&&(file, _, _)| file == node)
            .map_or(ROOT, |&(_, _, parent)| parent)
    }

    fn name(&self, node: u64) -> Name {
        Name::new(
            NODES
                .iter()
                .find(|&&(file, _, _)| file == node)
                .map_or(&[][..], |&(_, name, _)| name),
        )
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn generate(&self, node: u64) -> Result<Option<Vec<u8>>, super::StorageError> {
        let cpus = crate::cpu::CPUS.all();
        Ok(Some(match node {
            ONLINE => cpu_range(cpus.iter().filter(|cpu| cpu.is_online()).count()),
            POSSIBLE | PRESENT => cpu_range(cpus.len()),
            ENABLED => b"always madvise [never]\n".to_vec(),
            DEFRAG => b"always defer defer+madvise madvise [never]\n".to_vec(),
            _ => return Ok(None),
        }))
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let (node, name, _) = *NODES
            .iter()
            .filter(|&&(_, _, parent)| parent == directory)
            .nth(usize::try_from(position).ok()?)?;
        Some(DirectoryEntry {
            name: Name::new(name),
            node,
            kind: self.kind(node),
        })
    }
}
