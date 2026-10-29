//! `/etc/resolv.conf` (D157): a file system of one regular file, made when it is opened, that
//! lists the DNS servers the interface's DHCP lease names as `nameserver` lines, mounted over
//! the root's `/etc/resolv.conf` as Linux mounts one file over another. Programs' resolvers
//! (musl's, `nslookup`) read it; without a lease it is empty, and they fall back to their own
//! default.

use crate::vfs::{self, Attributes, DirectoryEntry, FileSystem, Name, NodeKind, StorageError};
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_net::ipv4::Ipv4Address;

/// Where the file is.
pub const PATH: &[u8] = b"/etc/resolv.conf";

const ROOT: u64 = 1;

const DEVICE: (u32, u32) = (0, 35);

/// The file's text for `servers`: one `nameserver` line for each.
pub fn text(servers: &[Option<Ipv4Address>]) -> Option<Vec<u8>> {
    let mut text = alloc::string::String::new();
    for server in servers.iter().flatten() {
        text.try_reserve(32).ok()?;
        writeln!(text, "nameserver {server}").ok()?;
    }
    Some(text.into_bytes())
}

/// The file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolvConf;

impl FileSystem for ResolvConf {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn kind(&self, _node: u64) -> NodeKind {
        NodeKind::Regular
    }

    fn attributes(&self, node: u64) -> Attributes {
        Attributes {
            kind: NodeKind::Regular,
            permissions: 0o644,
            file_id: node,
            links: 1,
            owner: 0,
            group: 0,
            size: 0,
            modified: 0,
        }
    }

    fn lookup(&self, _directory: u64, _name: &[u8]) -> Option<u64> {
        None
    }

    fn parent(&self, node: u64) -> u64 {
        node
    }

    fn name(&self, _node: u64) -> Name {
        Name::new(b"")
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn generate(&self, _node: u64) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(text(&super::dns_servers()))
    }

    fn entry(&self, _directory: u64, _position: u64) -> Option<DirectoryEntry> {
        None
    }
}

static FILE: ResolvConf = ResolvConf;

/// Mounts the file over `/etc/resolv.conf`, if the root has that file; once.
pub fn mount() {
    let _ = vfs::mount_file(PATH, &FILE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_lists_each_server_on_a_nameserver_line() {
        let servers = [
            Some(Ipv4Address::new(10, 0, 2, 3)),
            None,
            Some(Ipv4Address::new(192, 0, 2, 53)),
        ];
        assert_eq!(
            text(&servers).unwrap(),
            b"nameserver 10.0.2.3\nnameserver 192.0.2.53\n"
        );
        assert_eq!(text(&[None; 3]).unwrap(), b"");
        assert_eq!(ResolvConf.kind(ROOT), NodeKind::Regular);
        assert_eq!(ResolvConf.generate(ROOT), Ok(Some(Vec::new())));
    }
}
