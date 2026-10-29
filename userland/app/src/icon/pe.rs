//! A Windows program's icons, from Microsoft's public PE/COFF specification and resource
//! documentation: the resource directory (the optional header's third data directory), a tree
//! of type, name and language tables; the first `RT_GROUP_ICON` in its own order, first
//! language, lists its images by `RT_ICON` id, each a PNG or a bitmap entry. Resource
//! addresses are mapped to file offsets through the section table.

use super::{Candidate, Candidates, Format, bitmap, png_side};
use crate::reader::Reader;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const SUBDIRECTORY: u32 = 0x8000_0000;
const NAMED: u32 = 0x8000_0000;
const NEW_HEADER: u64 = 0x3c;
const COFF_BYTES: u64 = 24;
const PE32: u16 = 0x10b;
const PE32_PLUS: u16 = 0x20b;
const RESOURCE_DIRECTORY: u64 = 2;
const MAX_SECTIONS: usize = 96;
const MAX_ENTRIES: u32 = 1024;
const GROUP_HEADER: u64 = 6;
const GROUP_ENTRY: u64 = 14;
const BITMAP_RANK_TRUE_COLOUR: u8 = 0;
const BITMAP_RANK_OTHER: u8 = 3;
const PNG_RANK: u8 = 0;

fn u16_at(reader: &mut impl Reader, offset: u64) -> Option<u16> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact_at(offset, &mut bytes)
        .then(|| u16::from_le_bytes(bytes))
}

fn u32_at(reader: &mut impl Reader, offset: u64) -> Option<u32> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact_at(offset, &mut bytes)
        .then(|| u32::from_le_bytes(bytes))
}

struct Image {
    sections: u64,
    count: u16,
    resources: u32,
}

impl Image {
    fn read(reader: &mut impl Reader) -> Option<Self> {
        if u16_at(reader, 0)? != u16::from_le_bytes(*b"MZ") {
            return None;
        }
        let header = u64::from(u32_at(reader, NEW_HEADER)?);
        if u32_at(reader, header)? != u32::from_le_bytes(*b"PE\0\0") {
            return None;
        }
        let count = u16_at(reader, header + 6)?;
        let optional_size = u64::from(u16_at(reader, header + 20)?);
        let optional = header + COFF_BYTES;
        let (directories, count_at) = match u16_at(reader, optional)? {
            PE32 => (optional + 96, optional + 92),
            PE32_PLUS => (optional + 112, optional + 108),
            _ => return None,
        };
        if u64::from(u32_at(reader, count_at)?) <= RESOURCE_DIRECTORY {
            return None;
        }
        let resources = u32_at(reader, directories + 8 * RESOURCE_DIRECTORY)?;
        Some(Self {
            sections: optional + optional_size,
            count,
            resources,
        })
    }

    fn offset_of(&self, reader: &mut impl Reader, address: u32) -> Option<u64> {
        for index in 0..u64::from(self.count).min(MAX_SECTIONS as u64) {
            let at = self.sections + 40 * index;
            let virtual_size = u32_at(reader, at + 8)?;
            let start = u32_at(reader, at + 12)?;
            let raw_size = u32_at(reader, at + 16)?;
            let raw = u32_at(reader, at + 20)?;
            let length = virtual_size.max(raw_size);
            if address >= start && address - start < length {
                let inside = address - start;
                return (inside < raw_size).then_some(u64::from(raw) + u64::from(inside));
            }
        }
        None
    }

    fn table(&self, reader: &mut impl Reader, offset: u32) -> Option<(u64, u32)> {
        let at = self.offset_of(reader, self.resources.checked_add(offset)?)?;
        let named = u32::from(u16_at(reader, at + 12)?);
        let numbered = u32::from(u16_at(reader, at + 14)?);
        Some((at + 16, (named + numbered).min(MAX_ENTRIES)))
    }

    fn find(&self, reader: &mut impl Reader, table: u32, id: Option<u32>) -> Option<u32> {
        let (entries, count) = self.table(reader, table)?;
        (0..u64::from(count)).find_map(|index| {
            let name = u32_at(reader, entries + 8 * index)?;
            let target = u32_at(reader, entries + 8 * index + 4)?;
            let wanted = id.is_none_or(|id| name & NAMED == 0 && name == id);
            wanted.then_some(target)
        })
    }

    fn data(&self, reader: &mut impl Reader, kind: u32, id: Option<u32>) -> Option<(u64, u64)> {
        let types = self.find(reader, 0, Some(kind))?;
        let names = self.find(reader, types & !SUBDIRECTORY, id)?;
        let languages = self.find(reader, names & !SUBDIRECTORY, None)?;
        if types & SUBDIRECTORY == 0 || names & SUBDIRECTORY == 0 || languages & SUBDIRECTORY != 0 {
            return None;
        }
        let entry = self.offset_of(reader, self.resources.checked_add(languages)?)?;
        let address = u32_at(reader, entry)?;
        let size = u32_at(reader, entry + 4)?;
        Some((self.offset_of(reader, address)?, u64::from(size)))
    }
}

/// Adds the images of the program `reader` holds's first icon group to `list`.
pub fn candidates(reader: &mut impl Reader, list: &mut Candidates) {
    let Some(image) = Image::read(reader) else {
        return;
    };
    let Some((group, group_size)) = image.data(reader, RT_GROUP_ICON, None) else {
        return;
    };
    let Some(count) = u16_at(reader, group + 4) else {
        return;
    };
    for index in 0..u64::from(count) {
        let at = GROUP_HEADER + GROUP_ENTRY * index;
        if at + GROUP_ENTRY > group_size {
            break;
        }
        let Some(id) = u16_at(reader, group + at + 12) else {
            break;
        };
        let Some((offset, length)) = image.data(reader, RT_ICON, Some(u32::from(id))) else {
            continue;
        };
        if offset + length > reader.size() {
            continue;
        }
        if let Some(side) = png_side(reader, offset) {
            list.push(Candidate {
                side,
                rank: PNG_RANK,
                offset,
                length,
                format: Format::Png,
            });
        } else if let Some(info) = bitmap::info(reader, offset) {
            list.push(Candidate {
                side: info.side,
                rank: match info.bits {
                    32 => BITMAP_RANK_TRUE_COLOUR,
                    _ => BITMAP_RANK_OTHER,
                },
                offset,
                length,
                format: Format::Bitmap,
            });
        }
    }
}

#[cfg(test)]
pub(crate) mod samples {
    const RAW: u32 = 0x200;
    const ADDRESS: u32 = 0x1000;

    fn put(file: &mut [u8], at: usize, bytes: &[u8]) {
        file[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn table(out: &mut Vec<u8>, entries: &[(u32, u32)]) {
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for &(name, target) in entries {
            out.extend_from_slice(&name.to_le_bytes());
            out.extend_from_slice(&target.to_le_bytes());
        }
    }

    /// A PE32+ program whose resources hold one icon group listing `images` (id, data) in
    /// order.
    pub fn program_with_icons(images: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut resources = Vec::new();
        let count = images.len() as u32;
        let icon_names = 16 + 2 * 8;
        let group_names = icon_names + 16 + 8 * count;
        let icon_languages = group_names + 16 + 8;
        let group_language = icon_languages + count * (16 + 8);
        let data_entries = group_language + 16 + 8;
        let payload = data_entries + 16 * (count + 1);
        table(
            &mut resources,
            &[
                (3, 0x8000_0000 | icon_names),
                (14, 0x8000_0000 | group_names),
            ],
        );
        table(
            &mut resources,
            &images
                .iter()
                .enumerate()
                .map(|(index, (id, _))| {
                    (
                        u32::from(*id),
                        0x8000_0000 | (icon_languages + index as u32 * 24),
                    )
                })
                .collect::<Vec<_>>(),
        );
        table(&mut resources, &[(1, 0x8000_0000 | group_language)]);
        for index in 0..count {
            table(&mut resources, &[(0x409, data_entries + 16 * index)]);
        }
        table(&mut resources, &[(0x409, data_entries + 16 * count)]);
        let mut group = Vec::new();
        group.extend_from_slice(&[0, 0, 1, 0]);
        group.extend_from_slice(&(count as u16).to_le_bytes());
        for (id, data) in images {
            group.extend_from_slice(&[0; 8]);
            group.extend_from_slice(&(data.len() as u32).to_le_bytes());
            group.extend_from_slice(&id.to_le_bytes());
        }
        let mut blobs: Vec<&[u8]> = images.iter().map(|(_, data)| data.as_slice()).collect();
        blobs.push(&group);
        let mut at = payload;
        let mut entries = Vec::new();
        for blob in &blobs {
            entries.extend_from_slice(&(ADDRESS + at).to_le_bytes());
            entries.extend_from_slice(&(blob.len() as u32).to_le_bytes());
            entries.extend_from_slice(&[0; 8]);
            at += blob.len() as u32;
        }
        assert_eq!(resources.len() as u32, data_entries);
        resources.extend_from_slice(&entries);
        for blob in &blobs {
            resources.extend_from_slice(blob);
        }
        let mut file = crate::program::samples::windows_program(0x8664);
        put(&mut file, 0x86, &1u16.to_le_bytes());
        put(&mut file, 0x94, &240u16.to_le_bytes());
        put(&mut file, 0x98 + 108, &16u32.to_le_bytes());
        put(&mut file, 0x98 + 128, &ADDRESS.to_le_bytes());
        put(
            &mut file,
            0x98 + 132,
            &(resources.len() as u32).to_le_bytes(),
        );
        let section = 0x98 + 240;
        put(&mut file, section, b".rsrc\0\0\0");
        put(
            &mut file,
            section + 8,
            &(resources.len() as u32).to_le_bytes(),
        );
        put(&mut file, section + 12, &ADDRESS.to_le_bytes());
        put(
            &mut file,
            section + 16,
            &(resources.len() as u32).to_le_bytes(),
        );
        put(&mut file, section + 20, &RAW.to_le_bytes());
        file.resize(RAW as usize, 0);
        file.extend_from_slice(&resources);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::super::bitmap::samples::bitmap;
    use super::samples::program_with_icons;
    use super::*;
    use crate::reader::Bytes;

    #[test]
    fn the_first_group_s_images_are_found_through_the_resource_tree() {
        let small = bitmap(16, 32, &[], &vec![vec![0; 64]; 16], &vec![vec![0; 4]; 16]);
        let paletted = bitmap(
            32,
            4,
            &[0x00ff_0000; 16],
            &vec![vec![0; 16]; 32],
            &vec![vec![0; 4]; 32],
        );
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&256u32.to_be_bytes());
        png.extend_from_slice(&256u32.to_be_bytes());
        let program = program_with_icons(&[(1, small), (2, paletted), (7, png)]);
        let mut list = Candidates::new();
        candidates(&mut Bytes(&program), &mut list);
        let found: Vec<(u32, u8, Format)> = list
            .iter()
            .map(|candidate| (candidate.side, candidate.rank, candidate.format))
            .collect();
        assert_eq!(
            found,
            [
                (16, BITMAP_RANK_TRUE_COLOUR, Format::Bitmap),
                (32, BITMAP_RANK_OTHER, Format::Bitmap),
                (256, PNG_RANK, Format::Png)
            ]
        );
    }

    #[test]
    fn a_program_without_resources_or_not_a_program_has_no_icons() {
        let mut list = Candidates::new();
        let plain = crate::program::samples::windows_program(0x8664);
        candidates(&mut Bytes(&plain), &mut list);
        candidates(&mut Bytes(b"\x7fELF not windows"), &mut list);
        candidates(&mut Bytes(&[]), &mut list);
        assert_eq!(list.len(), 0);
    }
}
