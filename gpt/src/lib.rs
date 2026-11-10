//! GUID partition tables (UEFI 2.10, chapter 5): reading a disk's partitions, and writing a new
//! table with a protective MBR, for the kernel, the installer and `cargo xtask` (D238).

#![cfg_attr(not(test), no_std)]

/// A GUID in the byte order GPT stores it (the first three fields little-endian).
pub type Guid = [u8; 16];

/// The EFI system partition's type.
pub const EFI_SYSTEM_PARTITION: Guid = guid(0xc12a_7328, 0xf81f, 0x11d2, [0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b]);

/// EggOS's data partition, holding the EggFS volume an installed system mounts (D238).
pub const EGGOS_DATA_PARTITION: Guid = guid(0x0f3a_6e9c, 0x6b1d, 0x4f63, [0x9a, 0x2e, 0x5c, 0x8d, 0x7e, 0x4b, 0x2f, 0x10]);

/// Builds a GUID from its textual fields.
pub const fn guid(first: u32, second: u16, third: u16, rest: [u8; 8]) -> Guid {
    let a = first.to_le_bytes();
    let b = second.to_le_bytes();
    let c = third.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], rest[0], rest[1], rest[2], rest[3], rest[4],
        rest[5], rest[6], rest[7],
    ]
}

pub const SIGNATURE: &[u8; 8] = b"EFI PART";
pub const HEADER_BYTES: usize = 92;
pub const ENTRY_BYTES: usize = 128;
pub const ENTRIES: usize = 128;
pub const ENTRY_ARRAY_BYTES: usize = ENTRY_BYTES * ENTRIES;
const REVISION_1_0: u32 = 0x0001_0000;
const PROTECTIVE_MBR_TYPE: u8 = 0xee;
const NAME_UNITS: usize = 36;

/// CRC-32 as GPT and zip use it (IEEE 802.3, reflected polynomial `0xEDB88320`).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low = crc & 1;
            crc >>= 1;
            if low != 0 {
                crc ^= 0xedb8_8320;
            }
        }
    }
    !crc
}

/// One partition of a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub kind: Guid,
    pub unique: Guid,
    /// First and last logical block, inclusive.
    pub first: u64,
    pub last: u64,
    pub attributes: u64,
}

impl Partition {
    /// How many logical blocks it holds.
    pub fn blocks(&self) -> u64 {
        self.last.saturating_sub(self.first).saturating_add(1)
    }
}

/// A table's header, as read and checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub current: u64,
    pub backup: u64,
    pub first_usable: u64,
    pub last_usable: u64,
    pub disk: Guid,
    pub entries_at: u64,
    pub entry_count: u32,
    pub entry_bytes: u32,
    pub entries_crc: u32,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// The header in `block` (one logical block), if it is a valid GPT header.
pub fn read_header(block: &[u8]) -> Option<Header> {
    if block.get(..8)? != SIGNATURE {
        return None;
    }
    let size = u32_at(block, 12)? as usize;
    if !(HEADER_BYTES..=block.len()).contains(&size) {
        return None;
    }
    let mut copy = [0u8; 512];
    let header = copy.get_mut(..size)?;
    header.copy_from_slice(&block[..size]);
    header[16..20].fill(0);
    if crc32(header) != u32_at(block, 16)? {
        return None;
    }
    let entry_bytes = u32_at(block, 84)?;
    let entry_count = u32_at(block, 80)?;
    if entry_bytes < ENTRY_BYTES as u32 || entry_count == 0 || entry_count > 1024 {
        return None;
    }
    Some(Header {
        current: u64_at(block, 24)?,
        backup: u64_at(block, 32)?,
        first_usable: u64_at(block, 40)?,
        last_usable: u64_at(block, 48)?,
        disk: block.get(56..72)?.try_into().ok()?,
        entries_at: u64_at(block, 72)?,
        entry_count,
        entry_bytes,
        entries_crc: u32_at(block, 88)?,
    })
}

/// The partitions an entry array lists, checked against the header's CRC: the used entries.
pub fn partitions<'a>(header: &Header, entries: &'a [u8]) -> Option<impl Iterator<Item = Partition> + 'a> {
    let length = header.entry_count as usize * header.entry_bytes as usize;
    let array = entries.get(..length)?;
    if crc32(array) != header.entries_crc {
        return None;
    }
    let stride = header.entry_bytes as usize;
    let last_usable = header.last_usable;
    Some(array.chunks_exact(stride).filter_map(move |entry| {
        let kind: Guid = entry.get(..16)?.try_into().ok()?;
        if kind == [0; 16] {
            return None;
        }
        let partition = Partition {
            kind,
            unique: entry.get(16..32)?.try_into().ok()?,
            first: u64_at(entry, 32)?,
            last: u64_at(entry, 40)?,
            attributes: u64_at(entry, 48)?,
        };
        (partition.first <= partition.last && partition.last <= last_usable).then_some(partition)
    }))
}

/// A partition to write into a new table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewPartition<'a> {
    pub kind: Guid,
    pub unique: Guid,
    pub first: u64,
    pub last: u64,
    pub name: &'a str,
}

/// Where a new table's parts go on a disk of `blocks` logical blocks of `block_bytes` each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub block_bytes: u64,
    pub blocks: u64,
    pub entry_blocks: u64,
    pub first_usable: u64,
    pub last_usable: u64,
}

impl Layout {
    pub fn new(blocks: u64, block_bytes: u64) -> Option<Self> {
        let entry_blocks = (ENTRY_ARRAY_BYTES as u64).div_ceil(block_bytes);
        let first_usable = 2 + entry_blocks;
        let last_usable = blocks.checked_sub(2 + entry_blocks)?;
        (last_usable > first_usable).then_some(Self {
            block_bytes,
            blocks,
            entry_blocks,
            first_usable,
            last_usable,
        })
    }

    /// The first block at or after `block` aligned to 1 MiB, as every partitioning tool aligns.
    pub fn aligned(&self, block: u64) -> u64 {
        let per_mebibyte = (1 << 20) / self.block_bytes.max(1);
        block.next_multiple_of(per_mebibyte.max(1))
    }
}

/// The entry array for `partitions`.
pub fn entry_array(partitions: &[NewPartition<'_>]) -> [u8; ENTRY_ARRAY_BYTES] {
    let mut array = [0u8; ENTRY_ARRAY_BYTES];
    for (entry, partition) in array.chunks_exact_mut(ENTRY_BYTES).zip(partitions) {
        entry[..16].copy_from_slice(&partition.kind);
        entry[16..32].copy_from_slice(&partition.unique);
        entry[32..40].copy_from_slice(&partition.first.to_le_bytes());
        entry[40..48].copy_from_slice(&partition.last.to_le_bytes());
        for (index, unit) in partition.name.encode_utf16().take(NAME_UNITS).enumerate() {
            entry[56 + index * 2..58 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    array
}

/// A header for `layout`, the primary when `primary`, over the entry array whose CRC is
/// `entries_crc`.
pub fn header(layout: &Layout, disk: Guid, entries_crc: u32, primary: bool) -> [u8; HEADER_BYTES] {
    let mut header = [0u8; HEADER_BYTES];
    let last = layout.blocks - 1;
    let (current, backup, entries_at) = if primary {
        (1, last, 2)
    } else {
        (last, 1, last - layout.entry_blocks)
    };
    header[..8].copy_from_slice(SIGNATURE);
    header[8..12].copy_from_slice(&REVISION_1_0.to_le_bytes());
    header[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    header[24..32].copy_from_slice(&current.to_le_bytes());
    header[32..40].copy_from_slice(&backup.to_le_bytes());
    header[40..48].copy_from_slice(&layout.first_usable.to_le_bytes());
    header[48..56].copy_from_slice(&layout.last_usable.to_le_bytes());
    header[56..72].copy_from_slice(&disk);
    header[72..80].copy_from_slice(&entries_at.to_le_bytes());
    header[80..84].copy_from_slice(&(ENTRIES as u32).to_le_bytes());
    header[84..88].copy_from_slice(&(ENTRY_BYTES as u32).to_le_bytes());
    header[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&header);
    header[16..20].copy_from_slice(&crc.to_le_bytes());
    header
}

/// The protective MBR for a disk of `blocks` logical blocks: one partition of type `0xEE` over
/// the whole disk, as far as 32 bits reach.
pub fn protective_mbr(blocks: u64) -> [u8; 512] {
    let mut mbr = [0u8; 512];
    let entry = &mut mbr[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = PROTECTIVE_MBR_TYPE;
    entry[5..8].copy_from_slice(&[0xff, 0xff, 0xff]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let size = u32::try_from(blocks.saturating_sub(1)).unwrap_or(u32::MAX);
    entry[12..16].copy_from_slice(&size.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    mbr
}

/// A new table's writes: each `(block, bytes)` the caller writes, padded by it to whole blocks.
#[derive(Debug)]
pub struct NewTable {
    pub mbr: [u8; 512],
    pub primary_header: [u8; HEADER_BYTES],
    pub entries: [u8; ENTRY_ARRAY_BYTES],
    pub backup_header: [u8; HEADER_BYTES],
    pub layout: Layout,
}

impl NewTable {
    pub fn new(layout: Layout, disk: Guid, partitions: &[NewPartition<'_>]) -> Option<Self> {
        let fits = partitions.len() <= ENTRIES
            && partitions
                .iter()
                .all(|partition| partition.first >= layout.first_usable && partition.last <= layout.last_usable && partition.first <= partition.last);
        if !fits {
            return None;
        }
        let entries = entry_array(partitions);
        let entries_crc = crc32(&entries);
        Some(Self {
            mbr: protective_mbr(layout.blocks),
            primary_header: header(&layout, disk, entries_crc, true),
            entries,
            backup_header: header(&layout, disk, entries_crc, false),
            layout,
        })
    }

    /// Block numbers of the parts: the primary header, the primary entries, the backup entries
    /// and the backup header (the MBR is block 0).
    pub fn places(&self) -> [u64; 4] {
        let last = self.layout.blocks - 1;
        [1, 2, last - self.layout.entry_blocks, last]
    }
}

/// A version 4 GUID from 16 random bytes.
pub fn random_guid(mut bytes: [u8; 16]) -> Guid {
    bytes[7] = (bytes[7] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_its_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn the_efi_system_partition_type_has_gpt_byte_order() {
        assert_eq!(&EFI_SYSTEM_PARTITION[..4], &[0x28, 0x73, 0x2a, 0xc1]);
    }

    #[test]
    fn a_written_table_reads_back() {
        let layout = Layout::new(1 << 21, 512).unwrap();
        let esp_first = layout.aligned(layout.first_usable);
        let partitions = [
            NewPartition {
                kind: EFI_SYSTEM_PARTITION,
                unique: [1; 16],
                first: esp_first,
                last: esp_first + 2047,
                name: "EFI system partition",
            },
            NewPartition {
                kind: EGGOS_DATA_PARTITION,
                unique: [2; 16],
                first: layout.aligned(esp_first + 2048),
                last: layout.last_usable,
                name: "EggOS",
            },
        ];
        let table = NewTable::new(layout, [9; 16], &partitions).unwrap();
        let mut block = [0u8; 512];
        block[..HEADER_BYTES].copy_from_slice(&table.primary_header);
        let header = read_header(&block).unwrap();
        assert_eq!(header.entries_at, 2);
        assert_eq!(header.backup, (1 << 21) - 1);
        let found: Vec<_> = partitions_of(&header, &table.entries);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].kind, EGGOS_DATA_PARTITION);
        assert_eq!(found[0].blocks(), 2048);
        let mut backup = [0u8; 512];
        backup[..HEADER_BYTES].copy_from_slice(&table.backup_header);
        assert_eq!(read_header(&backup).unwrap().current, (1 << 21) - 1);
        assert_eq!(table.mbr[450], 0xee);
    }

    fn partitions_of(header: &Header, entries: &[u8]) -> Vec<Partition> {
        partitions(header, entries).unwrap().collect()
    }

    #[test]
    fn a_damaged_header_or_array_is_refused() {
        let layout = Layout::new(1 << 16, 512).unwrap();
        let table = NewTable::new(layout, [9; 16], &[]).unwrap();
        let mut block = [0u8; 512];
        block[..HEADER_BYTES].copy_from_slice(&table.primary_header);
        block[30] ^= 1;
        assert_eq!(read_header(&block), None);
        block[30] ^= 1;
        let header = read_header(&block).unwrap();
        let mut entries = table.entries;
        entries[0] = 1;
        assert!(partitions(&header, &entries).is_none());
    }

    #[test]
    fn a_partition_outside_the_usable_blocks_is_not_written() {
        let layout = Layout::new(1 << 16, 512).unwrap();
        let outside = NewPartition {
            kind: EGGOS_DATA_PARTITION,
            unique: [2; 16],
            first: 1,
            last: 10,
            name: "x",
        };
        assert!(NewTable::new(layout, [9; 16], &[outside]).is_none());
    }
}
