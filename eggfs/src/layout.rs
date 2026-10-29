use crate::checksum::{Crc32c, crc32c};

pub const BLOCK_SIZE: usize = 4096;
pub const INODE_SIZE: usize = 128;
pub const INODES_PER_BLOCK: u32 = (BLOCK_SIZE / INODE_SIZE) as u32;
pub const BITS_PER_BLOCK: u32 = (BLOCK_SIZE * 8) as u32;
pub const POINTERS_PER_BLOCK: u64 = (BLOCK_SIZE / 4) as u64;
pub const DIRECT_POINTERS: usize = 12;
pub const ROOT_INODE: u32 = 1;
pub const FIRST_FREE_INODE: u32 = 2;
pub const MAGIC: [u8; 8] = *b"EGGFS\0\0\0";
pub const FORMATTING_MAGIC: [u8; 8] = *b"EGGFSFMT";
/// The on-disk format version [`crate::format`] writes; version 1 volumes still mount and are
/// upgraded by the first change that needs version 2.
pub const VERSION: u32 = 2;
pub const OLDEST_VERSION: u32 = 1;
/// The longest target a symbolic link holds, in bytes.
pub const MAX_LINK_TARGET: usize = 4095;
/// The most names a regular file or a symbolic link has.
pub const MAX_LINKS: u32 = 65000;
pub const MIN_BLOCKS: u64 = 256;
pub const MAX_BLOCKS: u64 = u32::MAX as u64 - BITS_PER_BLOCK as u64;
pub const MIN_JOURNAL_BLOCKS: u32 = 64;
pub const MAX_JOURNAL_BLOCKS: u32 = 16384;
pub const JOURNAL_SHARE: u32 = 256;
pub const INODE_SHARE: u32 = 4;
pub const MAX_NAME: usize = 255;
pub const RECORD_HEADER: usize = 6;
pub const MAX_DIRECTORY_BLOCKS: u64 = 16384;
pub const DESCRIPTOR_MAGIC: [u8; 8] = *b"EGGFSJDS";
pub const COMMIT_MAGIC: [u8; 8] = *b"EGGFSJCM";
pub const DESCRIPTOR_HEADER: usize = 24;
pub const DESCRIPTOR_CAPACITY: u32 = ((BLOCK_SIZE - DESCRIPTOR_HEADER - 4) / 4) as u32;

pub type Block = [u8; BLOCK_SIZE];

pub fn zeroed_block() -> alloc::boxed::Box<Block> {
    alloc::vec![0u8; BLOCK_SIZE]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!())
}

pub fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

pub fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

pub fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32_at(bytes, at)) | u64::from(u32_at(bytes, at + 4)) << 32
}

pub fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub version: u32,
    pub blocks: u32,
    pub inodes: u32,
    pub block_bitmap: u32,
    pub block_bitmap_blocks: u32,
    pub inode_bitmap: u32,
    pub inode_bitmap_blocks: u32,
    pub journal: u32,
    pub journal_blocks: u32,
    pub inode_table: u32,
    pub inode_table_blocks: u32,
    pub data_start: u32,
    pub created: u64,
}

const SUPERBLOCK_CHECKSUM: usize = BLOCK_SIZE - 4;

impl Geometry {
    pub fn for_disk(disk_blocks: u64, created: u64) -> Option<Self> {
        if disk_blocks < MIN_BLOCKS {
            return None;
        }
        let blocks = disk_blocks.min(MAX_BLOCKS) as u32;
        let inodes = (blocks / INODE_SHARE)
            .max(INODES_PER_BLOCK)
            .div_ceil(INODES_PER_BLOCK)
            .saturating_mul(INODES_PER_BLOCK);
        let journal_blocks = (blocks / JOURNAL_SHARE).clamp(MIN_JOURNAL_BLOCKS, MAX_JOURNAL_BLOCKS);
        let block_bitmap_blocks = blocks.div_ceil(BITS_PER_BLOCK);
        let inode_bitmap_blocks = inodes.div_ceil(BITS_PER_BLOCK);
        let inode_table_blocks = inodes / INODES_PER_BLOCK;
        let block_bitmap = 1;
        let inode_bitmap = block_bitmap + block_bitmap_blocks;
        let journal = inode_bitmap + inode_bitmap_blocks;
        let inode_table = journal + journal_blocks;
        let data_start = inode_table.checked_add(inode_table_blocks)?;
        let geometry = Self {
            version: VERSION,
            blocks,
            inodes,
            block_bitmap,
            block_bitmap_blocks,
            inode_bitmap,
            inode_bitmap_blocks,
            journal,
            journal_blocks,
            inode_table,
            inode_table_blocks,
            data_start,
            created,
        };
        geometry.is_consistent().then_some(geometry)
    }

    fn is_consistent(&self) -> bool {
        let regions = [
            (self.block_bitmap, self.block_bitmap_blocks),
            (self.inode_bitmap, self.inode_bitmap_blocks),
            (self.journal, self.journal_blocks),
            (self.inode_table, self.inode_table_blocks),
        ];
        let mut next = 1u64;
        for (start, length) in regions {
            if u64::from(start) != next || length == 0 {
                return false;
            }
            next += u64::from(length);
        }
        next == u64::from(self.data_start)
            && u64::from(self.blocks) >= MIN_BLOCKS
            && u64::from(self.blocks) <= MAX_BLOCKS
            && u64::from(self.data_start) + 8 <= u64::from(self.blocks)
            && self.inodes > FIRST_FREE_INODE
            && self.inodes.is_multiple_of(INODES_PER_BLOCK)
            && u64::from(self.block_bitmap_blocks) * u64::from(BITS_PER_BLOCK)
                >= u64::from(self.blocks)
            && u64::from(self.inode_bitmap_blocks) * u64::from(BITS_PER_BLOCK)
                >= u64::from(self.inodes)
            && self.inode_table_blocks == self.inodes / INODES_PER_BLOCK
            && self.journal_blocks >= MIN_JOURNAL_BLOCKS
    }

    pub fn transaction_capacity(&self) -> u32 {
        (self.journal_blocks - 2).min(DESCRIPTOR_CAPACITY)
    }

    pub fn is_data_block(&self, block: u32) -> bool {
        block >= self.data_start && block < self.blocks
    }

    pub fn inode_location(&self, inode: u32) -> (u32, usize) {
        (
            self.inode_table + inode / INODES_PER_BLOCK,
            (inode % INODES_PER_BLOCK) as usize * INODE_SIZE,
        )
    }

    pub fn encode(&self) -> alloc::boxed::Box<Block> {
        let mut block = zeroed_block();
        block[0..8].copy_from_slice(&MAGIC);
        put_u32(&mut block[..], 8, self.version);
        put_u32(&mut block[..], 12, BLOCK_SIZE as u32);
        put_u32(&mut block[..], 16, self.blocks);
        put_u32(&mut block[..], 20, self.inodes);
        let fields = [
            self.block_bitmap,
            self.block_bitmap_blocks,
            self.inode_bitmap,
            self.inode_bitmap_blocks,
            self.journal,
            self.journal_blocks,
            self.inode_table,
            self.inode_table_blocks,
            self.data_start,
        ];
        for (index, value) in fields.into_iter().enumerate() {
            put_u32(&mut block[..], 24 + 4 * index, value);
        }
        put_u64(&mut block[..], 64, self.created);
        let checksum = crc32c(&block[..SUPERBLOCK_CHECKSUM]);
        put_u32(&mut block[..], SUPERBLOCK_CHECKSUM, checksum);
        block
    }

    pub fn decode(block: &[u8]) -> Option<Self> {
        if block.len() < BLOCK_SIZE
            || block[0..8] != MAGIC
            || u32_at(block, 12) != BLOCK_SIZE as u32
        {
            return None;
        }
        let version = superblock_version(block)?;
        let field = |index: usize| u32_at(block, 24 + 4 * index);
        let geometry = Self {
            version,
            blocks: u32_at(block, 16),
            inodes: u32_at(block, 20),
            block_bitmap: field(0),
            block_bitmap_blocks: field(1),
            inode_bitmap: field(2),
            inode_bitmap_blocks: field(3),
            journal: field(4),
            journal_blocks: field(5),
            inode_table: field(6),
            inode_table_blocks: field(7),
            data_start: field(8),
            created: u64_at(block, 64),
        };
        geometry.is_consistent().then_some(geometry)
    }

    pub fn same_layout(&self, other: &Geometry) -> bool {
        Geometry {
            version: other.version,
            ..*self
        } == *other
    }
}

fn superblock_checksum_with(block: &[u8], version: u32) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(&block[..8]);
    crc.update(&version.to_le_bytes());
    crc.update(&block[12..SUPERBLOCK_CHECKSUM]);
    crc.finish()
}

fn superblock_version(block: &[u8]) -> Option<u32> {
    let stored = u32_at(block, 8);
    let known = OLDEST_VERSION..=VERSION;
    if !known.contains(&stored) {
        return None;
    }
    let checksum = u32_at(block, SUPERBLOCK_CHECKSUM);
    let matches = |version: u32| superblock_checksum_with(block, version) == checksum;
    core::iter::once(stored)
        .chain(known.filter(|&version| version != stored))
        .find(|&version| matches(version))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeKind {
    Free,
    Directory,
    Regular,
    SymbolicLink,
}

impl InodeKind {
    pub fn code(self) -> u8 {
        match self {
            InodeKind::Free => 0,
            InodeKind::Directory => 1,
            InodeKind::Regular => 2,
            InodeKind::SymbolicLink => 3,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(InodeKind::Free),
            1 => Some(InodeKind::Directory),
            2 => Some(InodeKind::Regular),
            3 => Some(InodeKind::SymbolicLink),
            _ => None,
        }
    }
}

const ORPHAN_FLAG: u8 = 1;
const MORE_NAMES_AT: usize = 104;
const NEXT_ORPHAN_AT: usize = 108;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inode {
    pub kind: InodeKind,
    pub permissions: u16,
    pub generation: u32,
    pub parent: u32,
    pub child_directories: u32,
    pub size: u64,
    pub modified: u64,
    pub entries: u32,
    pub owner: u32,
    pub group: u32,
    pub direct: [u32; DIRECT_POINTERS],
    pub indirect: [u32; 3],
    pub more_names: u32,
    pub orphan: bool,
    pub next_orphan: u32,
}

const INODE_CHECKSUM: usize = INODE_SIZE - 4;

impl Inode {
    pub fn new(kind: InodeKind, permissions: u16, parent: u32, generation: u32, now: u64) -> Self {
        Self {
            kind,
            permissions,
            generation,
            parent,
            child_directories: 0,
            size: 0,
            modified: now,
            entries: 0,
            owner: 0,
            group: 0,
            direct: [0; DIRECT_POINTERS],
            indirect: [0; 3],
            more_names: 0,
            orphan: false,
            next_orphan: 0,
        }
    }

    pub fn encode(&self, number: u32, out: &mut [u8]) {
        let out = &mut out[..INODE_SIZE];
        out.fill(0);
        out[0] = self.kind.code();
        out[1] = if self.orphan { ORPHAN_FLAG } else { 0 };
        put_u16(out, 2, self.permissions);
        put_u32(out, 4, self.generation);
        put_u32(out, 8, self.parent);
        put_u32(out, 12, self.child_directories);
        put_u64(out, 16, self.size);
        put_u64(out, 24, self.modified);
        put_u32(out, 32, self.entries);
        put_u32(out, 36, self.owner);
        put_u32(out, 40, self.group);
        for (index, &pointer) in self.direct.iter().chain(&self.indirect).enumerate() {
            put_u32(out, 44 + 4 * index, pointer);
        }
        put_u32(out, MORE_NAMES_AT, self.more_names);
        put_u32(out, NEXT_ORPHAN_AT, self.next_orphan);
        let checksum = inode_checksum(number, &out[..INODE_CHECKSUM]);
        put_u32(out, INODE_CHECKSUM, checksum);
    }

    pub fn decode(number: u32, bytes: &[u8]) -> Option<Self> {
        let bytes = &bytes[..INODE_SIZE];
        if u32_at(bytes, INODE_CHECKSUM) != inode_checksum(number, &bytes[..INODE_CHECKSUM]) {
            return None;
        }
        let kind = InodeKind::from_code(bytes[0])?;
        if bytes[1] & !ORPHAN_FLAG != 0 {
            return None;
        }
        let mut direct = [0u32; DIRECT_POINTERS];
        for (index, pointer) in direct.iter_mut().enumerate() {
            *pointer = u32_at(bytes, 44 + 4 * index);
        }
        let mut indirect = [0u32; 3];
        for (index, pointer) in indirect.iter_mut().enumerate() {
            *pointer = u32_at(bytes, 44 + 4 * (DIRECT_POINTERS + index));
        }
        Some(Self {
            kind,
            permissions: u16_at(bytes, 2) & 0o7777,
            generation: u32_at(bytes, 4),
            parent: u32_at(bytes, 8),
            child_directories: u32_at(bytes, 12),
            size: u64_at(bytes, 16),
            modified: u64_at(bytes, 24),
            entries: u32_at(bytes, 32),
            owner: u32_at(bytes, 36),
            group: u32_at(bytes, 40),
            direct,
            indirect,
            more_names: u32_at(bytes, MORE_NAMES_AT),
            orphan: bytes[1] & ORPHAN_FLAG != 0,
            next_orphan: u32_at(bytes, NEXT_ORPHAN_AT),
        })
    }
}

fn inode_checksum(number: u32, bytes: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(&number.to_le_bytes());
    crc.update(bytes);
    crc.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub inode: u32,
    pub kind: InodeKind,
    pub name: &'a [u8],
    pub offset: usize,
    pub length: usize,
}

pub fn record_length(name: &[u8]) -> usize {
    RECORD_HEADER + name.len()
}

pub fn record_at(block: &[u8], offset: usize) -> Result<Option<Record<'_>>, ()> {
    if offset + RECORD_HEADER > BLOCK_SIZE {
        return Ok(None);
    }
    let inode = u32_at(block, offset);
    if inode == 0 {
        return Ok(None);
    }
    let kind = match InodeKind::from_code(block[offset + 4]) {
        Some(InodeKind::Free) | None => return Err(()),
        Some(kind) => kind,
    };
    let name_length = usize::from(block[offset + 5]);
    let length = RECORD_HEADER + name_length;
    if name_length == 0 || offset + length > BLOCK_SIZE {
        return Err(());
    }
    Ok(Some(Record {
        inode,
        kind,
        name: &block[offset + RECORD_HEADER..offset + length],
        offset,
        length,
    }))
}

pub fn records_end(block: &[u8]) -> Result<usize, ()> {
    let mut offset = 0;
    while let Some(record) = record_at(block, offset)? {
        offset += record.length;
    }
    Ok(offset)
}

pub fn write_record(block: &mut [u8], offset: usize, inode: u32, kind: InodeKind, name: &[u8]) {
    put_u32(block, offset, inode);
    block[offset + 4] = kind.code();
    block[offset + 5] = name.len() as u8;
    block[offset + RECORD_HEADER..offset + RECORD_HEADER + name.len()].copy_from_slice(name);
}

pub fn remove_record(block: &mut [u8], offset: usize, length: usize, end: usize) {
    block.copy_within(offset + length..end, offset);
    block[end - length..end].fill(0);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub sequence: u64,
    pub targets: alloc::vec::Vec<u32>,
}

impl Descriptor {
    pub fn encode(&self) -> alloc::boxed::Box<Block> {
        let mut block = zeroed_block();
        block[0..8].copy_from_slice(&DESCRIPTOR_MAGIC);
        put_u64(&mut block[..], 8, self.sequence);
        put_u32(&mut block[..], 16, self.targets.len() as u32);
        for (index, &target) in self.targets.iter().enumerate() {
            put_u32(&mut block[..], DESCRIPTOR_HEADER + 4 * index, target);
        }
        let checksum = crc32c(&block[..BLOCK_SIZE - 4]);
        put_u32(&mut block[..], BLOCK_SIZE - 4, checksum);
        block
    }

    pub fn decode(block: &[u8]) -> Option<Self> {
        if block[0..8] != DESCRIPTOR_MAGIC
            || u32_at(block, BLOCK_SIZE - 4) != crc32c(&block[..BLOCK_SIZE - 4])
        {
            return None;
        }
        let count = u32_at(block, 16);
        if count == 0 || count > DESCRIPTOR_CAPACITY {
            return None;
        }
        let targets = (0..count as usize)
            .map(|index| u32_at(block, DESCRIPTOR_HEADER + 4 * index))
            .collect();
        Some(Self {
            sequence: u64_at(block, 8),
            targets,
        })
    }
}

pub fn encode_commit(sequence: u64, count: u32, checksum: u64) -> alloc::boxed::Box<Block> {
    let mut block = zeroed_block();
    block[0..8].copy_from_slice(&COMMIT_MAGIC);
    put_u64(&mut block[..], 8, sequence);
    put_u32(&mut block[..], 16, count);
    put_u64(&mut block[..], 20, checksum);
    let own = crc32c(&block[..28]);
    put_u32(&mut block[..], 28, own);
    block
}

pub fn commit_matches(block: &[u8], sequence: u64, count: u32, checksum: u64) -> bool {
    block[0..8] == COMMIT_MAGIC
        && u64_at(block, 8) == sequence
        && u32_at(block, 16) == count
        && u64_at(block, 20) == checksum
        && u32_at(block, 28) == crc32c(&block[..28])
}
