//! Where a class served out of process listens (D400): a Unix socket per class in the user's
//! own state folder, `~/.local/state/windows/com/{CLSID}`, so only the user's programs reach
//! it; and whole messages (`crate::com::wire`) sent and read on a connection.

use super::super::shell_folders;
use crate::com::guid::Guid;
use crate::com::wire;
use crate::linux;
use crate::text::PATH_CAPACITY;
use alloc::vec::Vec;

const AF_UNIX: u16 = 1;
const SUN_PATH_BYTES: usize = 108;
/// The bytes of a `sockaddr_un`.
pub const ADDRESS_BYTES: usize = 2 + SUN_PATH_BYTES;
const FOLDER: &[u8] = b"/.local/state/windows/com";
const MSG_NOSIGNAL: usize = 0x4000;

/// A class's socket: its path (NUL-terminated) and its `sockaddr_un`.
#[derive(Debug, Clone, Copy)]
pub struct Place {
    pub path: [u8; PATH_CAPACITY],
    pub path_length: usize,
    pub address: [u8; ADDRESS_BYTES],
    pub address_length: usize,
}

impl Place {
    /// The path with its NUL.
    pub fn path(&self) -> &[u8] {
        &self.path[..self.path_length]
    }

    /// The address's bytes.
    pub fn address(&self) -> &[u8] {
        &self.address[..self.address_length]
    }
}

/// Where `class` listens; `None` when the path is too long for a socket. With `make`, the
/// folder is made first.
pub fn place_of(class: &Guid, make: bool) -> Option<Place> {
    let mut path = [0u8; PATH_CAPACITY];
    let home = shell_folders::home(&mut path);
    let mut length = home;
    let folder_end = length + FOLDER.len();
    path.get_mut(length..folder_end)?.copy_from_slice(FOLDER);
    length = folder_end;
    if make {
        *path.get_mut(length)? = 0;
        let _ = shell_folders::make_folders(&path[..=length]);
    }
    *path.get_mut(length)? = b'/';
    length += 1;
    for unit in class.text() {
        *path.get_mut(length)? = u8::try_from(unit).ok()?;
        length += 1;
    }
    *path.get_mut(length)? = 0;
    if length >= SUN_PATH_BYTES {
        return None;
    }
    let mut address = [0u8; ADDRESS_BYTES];
    address[..2].copy_from_slice(&AF_UNIX.to_ne_bytes());
    address[2..2 + length].copy_from_slice(&path[..length]);
    Some(Place {
        path,
        path_length: length + 1,
        address,
        address_length: 2 + length + 1,
    })
}

/// Sends the whole message `message` on `fd`; whether it went.
pub fn send(fd: usize, message: &[u8]) -> bool {
    let mut rest = message;
    while !rest.is_empty() {
        match linux::send(fd, rest, MSG_NOSIGNAL) {
            Ok(0) | Err(_) => return false,
            Ok(sent) => rest = &rest[sent.min(rest.len())..],
        }
    }
    true
}

fn read_exact(fd: usize, out: &mut [u8]) -> bool {
    let mut filled = 0;
    while filled < out.len() {
        match linux::read(fd, &mut out[filled..]) {
            Ok(0) | Err(_) => return false,
            Ok(read) => filled += read,
        }
    }
    true
}

/// The next message's body on `fd`; `None` when the other side went or sent a malformed one.
pub fn receive(fd: usize) -> Option<Vec<u8>> {
    let mut head = [0u8; 4];
    if !read_exact(fd, &mut head) {
        return None;
    }
    let length = wire::body_length(head)?;
    let mut body = Vec::new();
    body.try_reserve_exact(length).ok()?;
    body.resize(length, 0);
    read_exact(fd, &mut body).then_some(body)
}
