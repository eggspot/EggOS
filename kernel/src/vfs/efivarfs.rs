//! `/sys/firmware/efi/efivars` (D781), Linux's efivarfs: one file per UEFI variable, named
//! `<name>-<vendor GUID>` (`BootOrder-8be4df61-93ca-11d2-aa0d-00e098032b8c`), holding the
//! variable's four bytes of attributes (little-endian) and then its data. Reading calls
//! `GetVariable`, each write sets the variable to the attributes and data it holds (with the
//! append bit, appends), creating a file makes a name the next write gives a variable, and
//! removing one deletes the variable. Listing walks `GetNextVariableName`. Everything goes to
//! the firmware through [`crate::uefi_runtime`]; nothing is kept but the names seen, so a node
//! keeps its number.

use super::{
    Attributes, DirectoryEntry, FileSystem, Name, NodeKind, Refused, StorageError, Usage, WriteAt,
};
use crate::sync::SpinLock;
use crate::uefi_runtime::{self, Guid, NOT_FOUND};
use alloc::vec::Vec;

const ROOT: u64 = 1;
const FIRST_VARIABLE: u64 = 2;
const DEVICE: (u32, u32) = (0, 22);
const GUID_TEXT_BYTES: usize = 36;
const ATTRIBUTE_BYTES: usize = 4;

static KNOWN: SpinLock<Vec<(Vec<u16>, Guid)>> = SpinLock::new(Vec::new());
static LISTING: SpinLock<Vec<u64>> = SpinLock::new(Vec::new());

/// The firmware's variables, mounted at `/sys/firmware/efi/efivars`.
#[derive(Debug)]
pub struct FirmwareVariables;

const fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'a' + value - 10,
    }
}

/// The GUID as text, its first three fields little-endian as UEFI stores them.
pub fn guid_text(guid: &Guid) -> [u8; GUID_TEXT_BYTES] {
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut text = [b'-'; GUID_TEXT_BYTES];
    let mut at = 0;
    for (index, &byte_at) in order.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            at += 1;
        }
        let byte = guid[byte_at];
        text[at] = hex_digit(byte >> 4);
        text[at + 1] = hex_digit(byte & 0xf);
        at += 2;
    }
    text
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// The variable a file name names: its name in UTF-16 and its GUID.
pub fn parse_name(file: &[u8]) -> Option<(Vec<u16>, Guid)> {
    let split = file.len().checked_sub(GUID_TEXT_BYTES + 1)?;
    if split == 0 || file[split] != b'-' {
        return None;
    }
    let text = &file[split + 1..];
    let digits: Vec<u8> = text.iter().copied().filter(|&byte| byte != b'-').collect();
    if digits.len() != 32 || [8, 13, 18, 23].iter().any(|&at| text[at] != b'-') {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (slot, pair) in bytes.iter_mut().zip(digits.chunks(2)) {
        *slot = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut guid = [0u8; 16];
    for (index, &byte_at) in order.iter().enumerate() {
        guid[byte_at] = bytes[index];
    }
    let name = core::str::from_utf8(&file[..split]).ok()?;
    Some((name.encode_utf16().collect(), guid))
}

fn file_name(name: &[u16], guid: &Guid) -> Vec<u8> {
    let mut file = alloc::string::String::new();
    for unit in char::decode_utf16(name.iter().copied()) {
        file.push(unit.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    file.push('-');
    let mut bytes = file.into_bytes();
    bytes.extend_from_slice(&guid_text(guid));
    bytes
}

fn node_of(name: &[u16], guid: &Guid) -> u64 {
    let mut known = KNOWN.lock();
    let index = match known.iter().position(|(seen, seen_guid)| seen == name && seen_guid == guid) {
        Some(index) => index,
        None => {
            known.push((name.to_vec(), *guid));
            known.len() - 1
        }
    };
    FIRST_VARIABLE + index as u64
}

fn variable(node: u64) -> Option<(Vec<u16>, Guid)> {
    let index = usize::try_from(node.checked_sub(FIRST_VARIABLE)?).ok()?;
    KNOWN.lock().get(index).cloned()
}

fn refused(status: usize) -> Refused {
    match status {
        NOT_FOUND => Refused::NotFound,
        _ => Refused::InvalidArgument,
    }
}

impl FileSystem for FirmwareVariables {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        DEVICE
    }

    fn usage(&self) -> Usage {
        Usage::without_blocks(super::magic::EFIVARFS, false)
    }

    fn kind(&self, node: u64) -> NodeKind {
        match node {
            ROOT => NodeKind::Directory,
            _ => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let kind = self.kind(node);
        let size = match variable(node) {
            Some((name, guid)) => uefi_runtime::get_variable(&name, &guid)
                .map_or(0, |(_, data)| (ATTRIBUTE_BYTES + data.len()) as u64),
            None => 0,
        };
        Attributes {
            kind,
            permissions: match kind {
                NodeKind::Directory => 0o755,
                _ => 0o644,
            },
            file_id: node,
            links: if kind == NodeKind::Directory { 2 } else { 1 },
            owner: 0,
            group: 0,
            size,
            modified: 0,
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        if directory != ROOT {
            return None;
        }
        let (name, guid) = parse_name(name)?;
        let node = node_of(&name, &guid);
        uefi_runtime::get_variable(&name, &guid).ok()?;
        Some(node)
    }

    fn parent(&self, _node: u64) -> u64 {
        ROOT
    }

    fn name(&self, node: u64) -> Name {
        match variable(node) {
            Some((name, guid)) => Name::new(&file_name(&name, &guid)),
            None => Name::new(&[]),
        }
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn generate(&self, node: u64) -> Result<Option<Vec<u8>>, StorageError> {
        let Some((name, guid)) = variable(node) else {
            return Ok(None);
        };
        let Ok((attributes, data)) = uefi_runtime::get_variable(&name, &guid) else {
            return Ok(Some(Vec::new()));
        };
        let mut contents = Vec::with_capacity(ATTRIBUTE_BYTES + data.len());
        contents.extend_from_slice(&attributes.to_le_bytes());
        contents.extend_from_slice(&data);
        Ok(Some(contents))
    }

    fn is_writable(&self) -> bool {
        true
    }

    fn write(
        &self,
        node: u64,
        _at: WriteAt,
        bytes: &[u8],
        _now: u64,
    ) -> Result<(usize, u64), Refused> {
        let (name, guid) = variable(node).ok_or(Refused::NotFound)?;
        let attributes = bytes
            .first_chunk::<ATTRIBUTE_BYTES>()
            .map(|bytes| u32::from_le_bytes(*bytes))
            .ok_or(Refused::InvalidArgument)?;
        uefi_runtime::set_variable(&name, &guid, attributes, &bytes[ATTRIBUTE_BYTES..])
            .map_err(refused)?;
        Ok((bytes.len(), bytes.len() as u64))
    }

    fn truncate(&self, _node: u64, _length: u64, _now: u64) -> Result<(), Refused> {
        Ok(())
    }

    fn create(
        &self,
        directory: u64,
        name: &[u8],
        kind: NodeKind,
        _permissions: u32,
        _now: u64,
    ) -> Result<u64, Refused> {
        if directory != ROOT || kind != NodeKind::Regular {
            return Err(Refused::NotPermitted);
        }
        let (name, guid) = parse_name(name).ok_or(Refused::InvalidArgument)?;
        if uefi_runtime::get_variable(&name, &guid).is_ok() {
            return Err(Refused::Exists);
        }
        Ok(node_of(&name, &guid))
    }

    fn remove(
        &self,
        directory: u64,
        name: &[u8],
        directories: bool,
        _now: u64,
    ) -> Result<(), Refused> {
        if directory != ROOT || directories {
            return Err(Refused::NotDirectory);
        }
        let (name, guid) = parse_name(name).ok_or(Refused::NotFound)?;
        let (attributes, _) = uefi_runtime::get_variable(&name, &guid).map_err(refused)?;
        uefi_runtime::set_variable(&name, &guid, attributes, &[]).map_err(refused)
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        if directory != ROOT {
            return None;
        }
        if position == 0 {
            let nodes: Vec<u64> = uefi_runtime::variable_names()
                .iter()
                .map(|(name, guid)| node_of(name, guid))
                .collect();
            *LISTING.lock() = nodes;
        }
        let node = *LISTING.lock().get(usize::try_from(position).ok()?)?;
        Some(DirectoryEntry {
            name: self.name(node),
            node,
            kind: NodeKind::Regular,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBAL: Guid = [
        0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b,
        0x8c,
    ];

    #[test]
    fn file_names_carry_the_guid_as_linux_writes_it() {
        assert_eq!(
            &guid_text(&GLOBAL),
            b"8be4df61-93ca-11d2-aa0d-00e098032b8c"
        );
        let file = file_name(&"BootOrder".encode_utf16().collect::<Vec<_>>(), &GLOBAL);
        assert_eq!(file, b"BootOrder-8be4df61-93ca-11d2-aa0d-00e098032b8c");
        let (name, guid) = parse_name(&file).unwrap();
        assert_eq!(name, "BootOrder".encode_utf16().collect::<Vec<_>>());
        assert_eq!(guid, GLOBAL);
        assert!(parse_name(b"BootOrder").is_none());
        assert!(parse_name(b"-8be4df61-93ca-11d2-aa0d-00e098032b8c").is_none());
    }
}
