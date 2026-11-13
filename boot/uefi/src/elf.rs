pub const MACHINE_X86_64: u16 = 62;
pub const MACHINE_AARCH64: u16 = 183;

pub const EGGOS_NOTE_NAME: &[u8] = b"EggOS";
pub const EGGOS_NOTE_UEFI_ENTRY: u32 = 1;

const HEADER_BYTES: usize = 64;
const PROGRAM_HEADER_BYTES: usize = 56;
const LOAD: u32 = 1;
const NOTE: u32 = 4;
const MAX_PROGRAM_HEADERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub offset: u64,
    pub physical: u64,
    pub file_bytes: u64,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    bytes: &'a [u8],
    pub machine: u16,
    pub entry: u64,
    program_headers: usize,
    program_header_count: usize,
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        let header = bytes
            .get(..HEADER_BYTES)
            .ok_or("the kernel file is shorter than an ELF header")?;
        if header[..4] != *b"\x7fELF" || header[4] != 2 || header[5] != 1 {
            return Err("the kernel file is not a little-endian 64-bit ELF image");
        }
        let truncated = "the kernel file's ELF header is cut short";
        let machine = u16_at(header, 18).ok_or(truncated)?;
        let entry = u64_at(header, 24).ok_or(truncated)?;
        let program_headers = usize::try_from(u64_at(header, 32).ok_or(truncated)?)
            .map_err(|_| "the kernel's program headers are out of reach")?;
        let entry_size = usize::from(u16_at(header, 54).ok_or(truncated)?);
        let program_header_count = usize::from(u16_at(header, 56).ok_or(truncated)?);
        if entry_size != PROGRAM_HEADER_BYTES || program_header_count > MAX_PROGRAM_HEADERS {
            return Err("the kernel's program headers have an unexpected size");
        }
        let end = program_headers
            .checked_add(program_header_count * PROGRAM_HEADER_BYTES)
            .ok_or("the kernel's program headers are out of reach")?;
        if end > bytes.len() {
            return Err("the kernel's program headers run past the file");
        }
        Ok(Self {
            bytes,
            machine,
            entry,
            program_headers,
            program_header_count,
        })
    }

    fn program_header(&self, index: usize) -> &'a [u8] {
        let start = self.program_headers + index * PROGRAM_HEADER_BYTES;
        &self.bytes[start..start + PROGRAM_HEADER_BYTES]
    }

    fn of_type(&self, kind: u32) -> impl Iterator<Item = Segment> + 'a {
        let image = *self;
        (0..self.program_header_count).filter_map(move |index| {
            let header = image.program_header(index);
            (u32_at(header, 0)? == kind).then_some(())?;
            Some(Segment {
                offset: u64_at(header, 8)?,
                physical: u64_at(header, 24)?,
                file_bytes: u64_at(header, 32)?,
                memory_bytes: u64_at(header, 40)?,
            })
        })
    }

    pub fn loads(&self) -> impl Iterator<Item = Segment> + 'a {
        self.of_type(LOAD)
            .filter(|segment| segment.memory_bytes != 0)
    }

    pub fn file_bytes(&self, segment: &Segment) -> Option<&'a [u8]> {
        let start = usize::try_from(segment.offset).ok()?;
        let length = usize::try_from(segment.file_bytes).ok()?;
        self.bytes.get(start..start.checked_add(length)?)
    }

    pub fn physical_extent(&self) -> Option<(u64, u64)> {
        let mut extent: Option<(u64, u64)> = None;
        for segment in self.loads() {
            if segment.file_bytes > segment.memory_bytes {
                return None;
            }
            let end = segment.physical.checked_add(segment.memory_bytes)?;
            extent = Some(match extent {
                Some((low, high)) => (low.min(segment.physical), high.max(end)),
                None => (segment.physical, end),
            });
        }
        extent
    }

    pub fn note(&self, name: &[u8], kind: u32) -> Option<&'a [u8]> {
        for segment in self.of_type(NOTE) {
            let mut notes = self.file_bytes(&segment)?;
            while notes.len() >= 12 {
                let name_size = u32_at(notes, 0)? as usize;
                let descriptor_size = u32_at(notes, 4)? as usize;
                let note_kind = u32_at(notes, 8)?;
                let name_end = 12usize.checked_add(name_size)?;
                let descriptor_start = name_end.checked_next_multiple_of(4)?;
                let descriptor_end = descriptor_start.checked_add(descriptor_size)?;
                let note_name = notes.get(12..name_end)?;
                let descriptor = notes.get(descriptor_start..descriptor_end)?;
                let name_matches = note_name.strip_suffix(&[0]).unwrap_or(note_name) == name;
                if name_matches && note_kind == kind {
                    return Some(descriptor);
                }
                let next = descriptor_end.checked_next_multiple_of(4)?;
                notes = notes.get(next..).unwrap_or_default();
            }
        }
        None
    }

    pub fn uefi_entry(&self) -> Option<u64> {
        let descriptor = self.note(EGGOS_NOTE_NAME, EGGOS_NOTE_UEFI_ENTRY)?;
        u64_at(descriptor, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_with(segments: &[(u32, u64, u64, u64, u64)], tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER_BYTES];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[18..20].copy_from_slice(&MACHINE_X86_64.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x10_0000u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&(HEADER_BYTES as u64).to_le_bytes());
        bytes[54..56].copy_from_slice(&(PROGRAM_HEADER_BYTES as u16).to_le_bytes());
        bytes[56..58].copy_from_slice(&(segments.len() as u16).to_le_bytes());
        for &(kind, offset, physical, file_bytes, memory_bytes) in segments {
            let mut header = [0u8; PROGRAM_HEADER_BYTES];
            header[..4].copy_from_slice(&kind.to_le_bytes());
            header[8..16].copy_from_slice(&offset.to_le_bytes());
            header[24..32].copy_from_slice(&physical.to_le_bytes());
            header[32..40].copy_from_slice(&file_bytes.to_le_bytes());
            header[40..48].copy_from_slice(&memory_bytes.to_le_bytes());
            bytes.extend_from_slice(&header);
        }
        bytes.extend_from_slice(tail);
        bytes
    }

    #[test]
    fn the_extent_covers_every_load_segment() {
        let bytes = image_with(
            &[
                (LOAD, 0, 0x10_0000, 0x10, 0x1000),
                (LOAD, 0, 0x20_0000, 0x10, 0x3000),
            ],
            &[],
        );
        let image = Image::parse(&bytes).unwrap();
        assert_eq!(image.machine, MACHINE_X86_64);
        assert_eq!(image.physical_extent(), Some((0x10_0000, 0x20_3000)));
        assert_eq!(image.loads().count(), 2);
    }

    #[test]
    fn the_uefi_entry_comes_from_the_eggos_note() {
        let header_end = HEADER_BYTES + PROGRAM_HEADER_BYTES;
        let mut note = Vec::new();
        note.extend_from_slice(&6u32.to_le_bytes());
        note.extend_from_slice(&8u32.to_le_bytes());
        note.extend_from_slice(&EGGOS_NOTE_UEFI_ENTRY.to_le_bytes());
        note.extend_from_slice(b"EggOS\0\0\0");
        note.extend_from_slice(&0x10_1234u64.to_le_bytes());
        let bytes = image_with(
            &[(
                NOTE,
                header_end as u64,
                0,
                note.len() as u64,
                note.len() as u64,
            )],
            &note,
        );
        let image = Image::parse(&bytes).unwrap();
        assert_eq!(image.uefi_entry(), Some(0x10_1234));
    }

    #[test]
    fn a_file_that_is_not_elf_is_refused() {
        assert!(Image::parse(&[0u8; 80]).is_err());
        assert!(Image::parse(b"\x7fELF").is_err());
    }

    #[test]
    fn a_segment_with_more_file_than_memory_bytes_has_no_extent() {
        let bytes = image_with(&[(LOAD, 0, 0x10_0000, 0x2000, 0x1000)], &[]);
        let image = Image::parse(&bytes).unwrap();
        assert_eq!(image.physical_extent(), None);
    }
}
