//! Version information in a file (D178), for version.dll: the `RT_VERSION` resource of a PE
//! file (an executable or a DLL, 32 or 64 bit), found by walking the file's resource directory
//! (type, name, language), and the values in its `VS_VERSIONINFO` block that `VerQueryValueW`
//! names with a path such as `\`, `\VarFileInfo\Translation` or
//! `\StringFileInfo\040904b0\FileVersion`.

const RT_VERSION: u32 = 16;
const RESOURCE_DIRECTORY: usize = 2;
const DIRECTORY_HEADER: usize = 16;
const DIRECTORY_ENTRY: usize = 8;
const SUBDIRECTORY: u32 = 0x8000_0000;
const MAX_ENTRIES: usize = 4096;
const NODE_HEADER: usize = 6;
const MAX_DEPTH: usize = 8;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

struct Sections<'a> {
    table: &'a [u8],
}

impl Sections<'_> {
    fn file_offset(&self, address: u32) -> Option<usize> {
        self.table.as_chunks::<40>().0.iter().find_map(|header| {
            let virtual_size = u32_at(header, 8)?;
            let virtual_address = u32_at(header, 12)?;
            let raw_size = u32_at(header, 16)?;
            let raw_offset = u32_at(header, 20)?;
            let size = virtual_size.max(raw_size);
            let offset = address
                .checked_sub(virtual_address)
                .filter(|offset| *offset < size)?;
            (offset < raw_size).then(|| raw_offset as usize + offset as usize)
        })
    }
}

fn first_entry(resources: &[u8], directory: usize, wanted: Option<u32>) -> Option<u32> {
    let named = usize::from(u16_at(resources, directory + 12)?);
    let numbered = usize::from(u16_at(resources, directory + 14)?);
    let count = (named + numbered).min(MAX_ENTRIES);
    (0..count).find_map(|index| {
        let at = directory + DIRECTORY_HEADER + index * DIRECTORY_ENTRY;
        let name = u32_at(resources, at)?;
        let target = u32_at(resources, at + 4)?;
        match wanted {
            Some(id) if name != id => None,
            _ => Some(target),
        }
    })
}

/// The bytes of the first `RT_VERSION` resource in the PE file `file`, if it has one.
pub fn version_resource(file: &[u8]) -> Option<&[u8]> {
    if !file.starts_with(b"MZ") {
        return None;
    }
    let header = u32_at(file, 0x3c)? as usize;
    if file.get(header..header.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let section_count = usize::from(u16_at(file, header + 6)?);
    let optional_size = usize::from(u16_at(file, header + 20)?);
    let optional = header + 24;
    let directories = match u16_at(file, optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return None,
    };
    let directory_count = u32_at(file, directories - 4)? as usize;
    if directory_count <= RESOURCE_DIRECTORY {
        return None;
    }
    let resource_address = u32_at(file, directories + RESOURCE_DIRECTORY * 8)?;
    let resource_size = u32_at(file, directories + RESOURCE_DIRECTORY * 8 + 4)? as usize;
    let table_start = optional.checked_add(optional_size)?;
    let sections = Sections {
        table: file.get(table_start..table_start.checked_add(section_count.checked_mul(40)?)?)?,
    };
    let start = sections.file_offset(resource_address)?;
    let resources = file.get(start..start.checked_add(resource_size)?.min(file.len()))?;
    let mut entry = first_entry(resources, 0, Some(RT_VERSION))?;
    for _ in 0..2 {
        if entry & SUBDIRECTORY == 0 {
            return None;
        }
        entry = first_entry(resources, (entry & !SUBDIRECTORY) as usize, None)?;
    }
    if entry & SUBDIRECTORY != 0 {
        return None;
    }
    let data_entry = entry as usize;
    let data_address = u32_at(resources, data_entry)?;
    let data_size = u32_at(resources, data_entry + 4)? as usize;
    let data = sections.file_offset(data_address)?;
    file.get(data..data.checked_add(data_size)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Node {
    start: usize,
    end: usize,
    value_start: usize,
    value_length: usize,
    text: bool,
    key_start: usize,
    key_end: usize,
    children: usize,
}

fn align4(at: usize) -> usize {
    at.next_multiple_of(4)
}

fn node(block: &[u8], start: usize, limit: usize) -> Option<Node> {
    let length = usize::from(u16_at(block, start)?);
    let value_length = usize::from(u16_at(block, start + 2)?);
    let text = u16_at(block, start + 4)? == 1;
    let end = start.checked_add(length)?.min(limit);
    if length < NODE_HEADER || end > block.len() {
        return None;
    }
    let key_start = start + NODE_HEADER;
    let mut key_end = key_start;
    while u16_at(block, key_end)? != 0 {
        key_end += 2;
        if key_end >= end {
            return None;
        }
    }
    let value_start = align4(key_end + 2);
    let value_bytes = if text { value_length * 2 } else { value_length };
    let children = align4(value_start + value_bytes).min(end);
    Some(Node {
        start,
        end,
        value_start,
        value_length,
        text,
        key_start,
        key_end,
        children,
    })
}

fn key_matches(block: &[u8], node: &Node, wanted: &[u16]) -> bool {
    let key = &block[node.key_start..node.key_end];
    key.len() == wanted.len() * 2
        && key
            .as_chunks::<2>()
            .0
            .iter()
            .zip(wanted)
            .all(|(pair, unit)| {
                crate::text::upper(u16::from_le_bytes(*pair)) == crate::text::upper(*unit)
            })
}

/// Where the value that `path` (UTF-16, components separated by `\`) names lies in the
/// version block `block`: its offset in the block, its length (characters for text, bytes
/// otherwise, as `VerQueryValueW` reports it) and whether it is text.
pub fn query(block: &[u8], path: &[u16]) -> Option<(usize, usize, bool)> {
    let mut current = node(block, 0, block.len())?;
    let components = path
        .split(|unit| *unit == u16::from(b'\\'))
        .filter(|component| !component.is_empty());
    for (depth, component) in components.enumerate() {
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut at = current.children;
        let found = loop {
            if at + NODE_HEADER > current.end {
                break None;
            }
            let child = node(block, at, current.end)?;
            if key_matches(block, &child, component) {
                break Some(child);
            }
            if child.end <= child.start {
                break None;
            }
            at = align4(child.end);
        };
        current = found?;
    }
    if current.value_start > current.end {
        return None;
    }
    Some((current.value_start, current.value_length, current.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_node(out: &mut Vec<u8>, key: &str, value: &[u8], text: bool, children: &[Vec<u8>]) {
        let start = out.len();
        out.extend_from_slice(&[0; 6]);
        for unit in key.encode_utf16().chain([0]) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(value);
        for child in children {
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            out.extend_from_slice(child);
        }
        let length = (out.len() - start) as u16;
        let value_length = (if text { value.len() / 2 } else { value.len() }) as u16;
        out[start..start + 2].copy_from_slice(&length.to_le_bytes());
        out[start + 2..start + 4].copy_from_slice(&value_length.to_le_bytes());
        out[start + 4..start + 6].copy_from_slice(&u16::from(text).to_le_bytes());
    }

    fn built(key: &str, value: &[u8], text: bool, children: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        put_node(&mut out, key, value, text, children);
        out
    }

    fn wide_bytes(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn sample() -> Vec<u8> {
        let mut fixed = vec![0u8; 52];
        fixed[0..4].copy_from_slice(&0xfeef_04bdu32.to_le_bytes());
        let version = built("FileVersion", &wide_bytes("2.0.31"), true, &[]);
        let name = built("ProductName", &wide_bytes("Egg"), true, &[]);
        let table = built("040904b0", &[], true, &[version, name]);
        let strings = built("StringFileInfo", &[], true, &[table]);
        let translation = built("Translation", &[0x09, 0x04, 0xb0, 0x04], false, &[]);
        let variables = built("VarFileInfo", &[], true, &[translation]);
        built("VS_VERSION_INFO", &fixed, false, &[strings, variables])
    }

    fn path(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn queries_find_the_fixed_part_strings_and_translations() {
        let block = sample();
        let (offset, length, text) = query(&block, &path("\\")).unwrap();
        assert_eq!((length, text), (52, false));
        assert_eq!(&block[offset..offset + 4], &0xfeef_04bdu32.to_le_bytes());
        let (offset, length, text) =
            query(&block, &path("\\StringFileInfo\\040904B0\\fileversion")).unwrap();
        assert_eq!((length, text), (7, true));
        assert_eq!(&block[offset..offset + 14], &wide_bytes("2.0.31")[..]);
        let (offset, length, _) =
            query(&block, &path("\\StringFileInfo\\040904b0\\ProductName")).unwrap();
        assert_eq!(length, 4);
        assert_eq!(&block[offset..offset + 8], &wide_bytes("Egg")[..]);
        let (offset, length, text) = query(&block, &path("\\VarFileInfo\\Translation")).unwrap();
        assert_eq!((length, text), (4, false));
        assert_eq!(&block[offset..offset + 4], &[0x09, 0x04, 0xb0, 0x04]);
        assert_eq!(
            query(&block, &path("\\StringFileInfo\\040904b0\\Missing")),
            None
        );
        assert_eq!(query(&block[..4], &path("\\")), None);
    }

    fn pe_with_version(resource: &[u8], optional_magic: u16) -> Vec<u8> {
        let mut file = vec![0u8; 0x400];
        file[0..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        file[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        let optional_size: u16 = if optional_magic == 0x20b { 240 } else { 224 };
        file[0x94..0x96].copy_from_slice(&optional_size.to_le_bytes());
        let optional = 0x98;
        file[optional..optional + 2].copy_from_slice(&optional_magic.to_le_bytes());
        let directories = optional + if optional_magic == 0x20b { 112 } else { 96 };
        file[directories - 4..directories].copy_from_slice(&16u32.to_le_bytes());
        let section_rva = 0x1000u32;
        let mut tree = vec![0u8; 0x80];
        let directory = |tree: &mut Vec<u8>, at: usize, id: u32, target: u32| {
            tree[at + 14..at + 16].copy_from_slice(&1u16.to_le_bytes());
            tree[at + 16..at + 20].copy_from_slice(&id.to_le_bytes());
            tree[at + 20..at + 24].copy_from_slice(&target.to_le_bytes());
        };
        directory(&mut tree, 0, RT_VERSION, SUBDIRECTORY | 0x18);
        directory(&mut tree, 0x18, 1, SUBDIRECTORY | 0x30);
        directory(&mut tree, 0x30, 0x409, 0x48);
        tree[0x48..0x4c].copy_from_slice(&(section_rva + 0x80).to_le_bytes());
        tree[0x4c..0x50].copy_from_slice(&(resource.len() as u32).to_le_bytes());
        tree.extend_from_slice(resource);
        let directory_entry = directories + RESOURCE_DIRECTORY * 8;
        file[directory_entry..directory_entry + 4].copy_from_slice(&section_rva.to_le_bytes());
        file[directory_entry + 4..directory_entry + 8]
            .copy_from_slice(&(tree.len() as u32).to_le_bytes());
        let section = optional + usize::from(optional_size);
        file[section..section + 6].copy_from_slice(b".rsrc\0");
        file[section + 8..section + 12].copy_from_slice(&(tree.len() as u32).to_le_bytes());
        file[section + 12..section + 16].copy_from_slice(&section_rva.to_le_bytes());
        file[section + 16..section + 20].copy_from_slice(&(tree.len() as u32).to_le_bytes());
        file[section + 20..section + 24].copy_from_slice(&0x400u32.to_le_bytes());
        file.extend_from_slice(&tree);
        file
    }

    #[test]
    fn the_version_resource_is_found_in_32_and_64_bit_files() {
        let block = sample();
        for magic in [0x10b, 0x20b] {
            let file = pe_with_version(&block, magic);
            assert_eq!(version_resource(&file), Some(&block[..]), "{magic:#x}");
        }
        let mut broken = pe_with_version(&block, 0x20b);
        broken[0x80] = b'X';
        assert_eq!(version_resource(&broken), None);
        assert_eq!(version_resource(b"MZ"), None);
        assert_eq!(version_resource(b"\x7fELF"), None);
    }
}
