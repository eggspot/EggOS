//! Shrinking a file system into its free tail (D782; installer dual boot v0.1 section 4, step
//! 3): an ext4 or NTFS volume is cut at a new end that only free space lies beyond, its new
//! size written last, so a failure halfway leaves it as it was or merely smaller than its
//! partition. Nothing is moved: the planner ([`crate::partitions::ext4_tail`],
//! [`crate::partitions::ntfs_tail`]) already measured how much free tail there is, and this
//! checks it again before writing.
//!
//! ext4 loses its trailing groups (their inodes must all be free; their bitmaps and inode
//! tables, which `flex_bg` keeps in earlier groups, are freed there) and the last group kept is
//! cut short, its bitmap padded as ext4 pads a short last group; every changed descriptor,
//! bitmap and the superblock get their checksums again (`metadata_csum`'s CRC-32C or the older
//! CRC-16). NTFS gets a new total in its boot sector and a backup boot sector at the new end,
//! and its volume is marked dirty so Windows checks it at its next start and trims `$Bitmap`
//! and `$BadClus` to the new size.

use crate::partitions::{le16, le32, le64, ntfs_attribute, ntfs_record};
use alloc::vec::Vec;
use eggos_eggfs::Crc32c;

/// Reads and writes inside one partition, at byte offsets from its start.
pub trait PartitionIo {
    fn read(&mut self, at: u64, buffer: &mut [u8]) -> bool;
    fn write(&mut self, at: u64, bytes: &[u8]) -> bool;
}

/// Why a file system was not shrunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShrinkError {
    /// Its layout or features are ones this does not shrink (ext4's `meta_bg` or `bigalloc`,
    /// an unclean or damaged volume).
    NotShrinkable,
    /// Something lies beyond the new end after all: used blocks or inodes.
    TailInUse,
    /// The disk could not be read.
    Read,
    /// The disk could not be written: the volume may be partly changed, never larger.
    Write,
}

const EXT4_SUPERBLOCK_AT: u64 = 1024;
const EXT4_SUPERBLOCK_BYTES: usize = 1024;
const EXT4_STATE_CLEAN: u16 = 1;
const EXT4_STATE_ERRORS: u16 = 2;
const EXT4_INCOMPAT_RECOVER: u32 = 0x4;
const EXT4_INCOMPAT_META_BG: u32 = 0x10;
const EXT4_INCOMPAT_64BIT: u32 = 0x80;
const EXT4_INCOMPAT_CSUM_SEED: u32 = 0x2000;
const EXT4_RO_COMPAT_GDT_CSUM: u32 = 0x10;
const EXT4_RO_COMPAT_BIGALLOC: u32 = 0x200;
const EXT4_RO_COMPAT_METADATA_CSUM: u32 = 0x400;
const EXT4_GROUP_INODE_UNINIT: u16 = 0x1;
const EXT4_GROUP_BLOCK_UNINIT: u16 = 0x2;
const EXT4_SMALL_DESCRIPTOR: usize = 32;
const EXT4_CHECKSUM_AT: usize = 0x1e;
const EXT4_SUPERBLOCK_CHECKSUM_AT: usize = 0x3fc;
const EXT4_MOST_BLOCK_SHIFT: u32 = 6;

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn crc32c_raw(register: u32, bytes: &[u8]) -> u32 {
    let mut crc = Crc32c::from_register(register);
    crc.update(bytes);
    crc.register()
}

fn crc16(mut crc: u16, bytes: &[u8]) -> u16 {
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = match crc & 1 {
                1 => (crc >> 1) ^ 0xa001,
                _ => crc >> 1,
            };
        }
    }
    crc
}

struct Ext4 {
    superblock: Vec<u8>,
    wide: bool,
    blocks: u64,
    first_data_block: u64,
    block_bytes: u64,
    per_group: u64,
    inodes_per_group: u64,
    inode_bytes: u64,
    descriptor_bytes: usize,
    metadata_checksums: bool,
    group_checksums: bool,
    seed: u32,
}

impl Ext4 {
    fn read(io: &mut dyn PartitionIo) -> Result<Self, ShrinkError> {
        let mut superblock = alloc::vec![0u8; EXT4_SUPERBLOCK_BYTES];
        if !io.read(EXT4_SUPERBLOCK_AT, &mut superblock) {
            return Err(ShrinkError::Read);
        }
        let field = |at: usize| le32(&superblock, at).ok_or(ShrinkError::NotShrinkable);
        let state = le16(&superblock, 58).ok_or(ShrinkError::NotShrinkable)?;
        let incompat = field(96)?;
        let ro_compat = field(100)?;
        if state & EXT4_STATE_CLEAN == 0
            || state & EXT4_STATE_ERRORS != 0
            || incompat & (EXT4_INCOMPAT_META_BG | EXT4_INCOMPAT_RECOVER) != 0
            || ro_compat & EXT4_RO_COMPAT_BIGALLOC != 0
        {
            return Err(ShrinkError::NotShrinkable);
        }
        let wide = incompat & EXT4_INCOMPAT_64BIT != 0;
        let high = |at: usize| -> Result<u64, ShrinkError> {
            Ok(match wide {
                true => u64::from(field(at)?) << 32,
                false => 0,
            })
        };
        let shift = field(24)?;
        if shift > EXT4_MOST_BLOCK_SHIFT {
            return Err(ShrinkError::NotShrinkable);
        }
        let descriptor_bytes = match wide {
            true => usize::from(le16(&superblock, 254).ok_or(ShrinkError::NotShrinkable)?)
                .max(EXT4_SMALL_DESCRIPTOR),
            false => EXT4_SMALL_DESCRIPTOR,
        };
        let metadata_checksums = ro_compat & EXT4_RO_COMPAT_METADATA_CSUM != 0;
        let seed = match incompat & EXT4_INCOMPAT_CSUM_SEED {
            0 => crc32c_raw(!0, &superblock[0x68..0x78]),
            _ => field(0x270)?,
        };
        let ext4 = Self {
            blocks: u64::from(field(4)?) | high(0x150)?,
            first_data_block: u64::from(field(20)?),
            block_bytes: 1024 << shift,
            per_group: u64::from(field(32)?),
            inodes_per_group: u64::from(field(40)?),
            inode_bytes: u64::from(le16(&superblock, 88).ok_or(ShrinkError::NotShrinkable)?),
            descriptor_bytes,
            metadata_checksums,
            group_checksums: !metadata_checksums && ro_compat & EXT4_RO_COMPAT_GDT_CSUM != 0,
            seed,
            wide,
            superblock,
        };
        if ext4.per_group == 0 || ext4.per_group > 8 * ext4.block_bytes || ext4.blocks == 0 {
            return Err(ShrinkError::NotShrinkable);
        }
        Ok(ext4)
    }

    fn groups(&self, blocks: u64) -> u64 {
        (blocks - self.first_data_block).div_ceil(self.per_group)
    }

    fn group_start(&self, group: u64) -> u64 {
        self.first_data_block + group * self.per_group
    }

    fn in_group(&self, group: u64, blocks: u64) -> u64 {
        self.per_group.min(blocks.saturating_sub(self.group_start(group)))
    }

    fn table_at(&self) -> u64 {
        (self.first_data_block + 1) * self.block_bytes
    }

    fn wide_field(&self, descriptor: &[u8], low: usize, high: usize, bits: u32) -> u64 {
        let low_value = match bits {
            16 => u64::from(le16(descriptor, low).unwrap_or(0)),
            _ => u64::from(le32(descriptor, low).unwrap_or(0)),
        };
        let high_value = match (self.descriptor_bytes > EXT4_SMALL_DESCRIPTOR, bits) {
            (false, _) => 0,
            (true, 16) => u64::from(le16(descriptor, high).unwrap_or(0)),
            (true, _) => u64::from(le32(descriptor, high).unwrap_or(0)),
        };
        low_value | (high_value << bits)
    }

    fn set_free_blocks(&self, descriptor: &mut [u8], free: u64) {
        put16(descriptor, 0x0c, free as u16);
        if self.descriptor_bytes > EXT4_SMALL_DESCRIPTOR {
            put16(descriptor, 0x2c, (free >> 16) as u16);
        }
    }

    fn bitmap_checksum(&self, descriptor: &mut [u8], bitmap: &[u8]) {
        if !self.metadata_checksums {
            return;
        }
        let length = (self.per_group / 8) as usize;
        let crc = crc32c_raw(self.seed, &bitmap[..length.min(bitmap.len())]);
        put16(descriptor, 0x18, crc as u16);
        if self.descriptor_bytes > EXT4_SMALL_DESCRIPTOR {
            put16(descriptor, 0x38, (crc >> 16) as u16);
        }
    }

    fn descriptor_checksum(&self, group: u64, descriptor: &mut [u8]) {
        let group_bytes = (group as u32).to_le_bytes();
        if self.metadata_checksums {
            put16(descriptor, EXT4_CHECKSUM_AT, 0);
            let crc = crc32c_raw(crc32c_raw(self.seed, &group_bytes), descriptor);
            put16(descriptor, EXT4_CHECKSUM_AT, crc as u16);
        } else if self.group_checksums {
            let mut crc = crc16(!0, &self.superblock[0x68..0x78]);
            crc = crc16(crc, &group_bytes);
            crc = crc16(crc, &descriptor[..EXT4_CHECKSUM_AT]);
            crc = crc16(crc, &descriptor[EXT4_CHECKSUM_AT + 2..]);
            put16(descriptor, EXT4_CHECKSUM_AT, crc);
        }
    }

    fn metadata_blocks(&self, descriptor: &[u8]) -> [(u64, u64); 3] {
        let table_blocks = (self.inodes_per_group * self.inode_bytes).div_ceil(self.block_bytes);
        [
            (self.wide_field(descriptor, 0x00, 0x20, 32), 1),
            (self.wide_field(descriptor, 0x04, 0x24, 32), 1),
            (self.wide_field(descriptor, 0x08, 0x28, 32), table_blocks),
        ]
    }
}

/// Shrinks the ext4 volume `io` holds to at most `new_bytes`: the bytes it now takes.
pub fn shrink_ext4(io: &mut dyn PartitionIo, new_bytes: u64) -> Result<u64, ShrinkError> {
    let fs = Ext4::read(io)?;
    let mut new_blocks = new_bytes / fs.block_bytes;
    if new_blocks >= fs.blocks {
        return Ok(fs.blocks * fs.block_bytes);
    }
    if new_blocks <= fs.first_data_block + 1 {
        return Err(ShrinkError::TailInUse);
    }
    let old_groups = fs.groups(fs.blocks);
    let mut table = alloc::vec![0u8; old_groups as usize * fs.descriptor_bytes];
    if !io.read(fs.table_at(), &mut table) {
        return Err(ShrinkError::Read);
    }
    let descriptor = |table: &[u8], group: u64| -> Vec<u8> {
        let at = group as usize * fs.descriptor_bytes;
        table[at..at + fs.descriptor_bytes].to_vec()
    };
    let last = fs.groups(new_blocks) - 1;
    let flags = le16(&descriptor(&table, last), 0x12).unwrap_or(0);
    if flags & EXT4_GROUP_BLOCK_UNINIT != 0 && new_blocks > fs.group_start(last) {
        new_blocks = fs.group_start(last);
    }
    let kept = fs.groups(new_blocks);
    let mut bitmap = alloc::vec![0u8; fs.block_bytes as usize];
    let mut freed_in_removed = 0u64;
    for group in (kept.saturating_sub(1))..old_groups {
        let entry = descriptor(&table, group);
        let start = fs.group_start(group);
        let in_old = fs.in_group(group, fs.blocks);
        let cut_from = new_blocks.saturating_sub(start).min(in_old);
        let free = fs.wide_field(&entry, 0x0c, 0x2c, 16);
        let group_flags = le16(&entry, 0x12).unwrap_or(0);
        if group >= kept {
            let free_inodes = fs.wide_field(&entry, 0x0e, 0x2e, 16);
            if group_flags & EXT4_GROUP_INODE_UNINIT == 0 && free_inodes != fs.inodes_per_group {
                return Err(ShrinkError::TailInUse);
            }
            freed_in_removed += free;
        }
        if group_flags & EXT4_GROUP_BLOCK_UNINIT != 0 {
            if cut_from == 0 && free != in_old {
                return Err(ShrinkError::TailInUse);
            }
            continue;
        }
        let bitmap_at = fs.metadata_blocks(&entry)[0].0 * fs.block_bytes;
        if !io.read(bitmap_at, &mut bitmap) {
            return Err(ShrinkError::Read);
        }
        let used_beyond = (cut_from..in_old).any(|bit| {
            let bit = bit as usize;
            bitmap[bit / 8] & (1 << (bit % 8)) != 0
        });
        if used_beyond {
            return Err(ShrinkError::TailInUse);
        }
    }
    let mut changed_bitmaps: Vec<(u64, Vec<u8>)> = Vec::new();
    let bitmap_of = |io: &mut dyn PartitionIo,
                         changed: &mut Vec<(u64, Vec<u8>)>,
                         group: u64,
                         table: &[u8]|
     -> Result<usize, ShrinkError> {
        if let Some(index) = changed.iter().position(|(seen, _)| *seen == group) {
            return Ok(index);
        }
        let entry = descriptor(table, group);
        let mut bytes = alloc::vec![0u8; fs.block_bytes as usize];
        if !io.read(fs.metadata_blocks(&entry)[0].0 * fs.block_bytes, &mut bytes) {
            return Err(ShrinkError::Read);
        }
        changed.push((group, bytes));
        Ok(changed.len() - 1)
    };
    let mut freed_in_kept = 0u64;
    for group in kept..old_groups {
        let entry = descriptor(&table, group);
        for (first, count) in fs.metadata_blocks(&entry) {
            for block in first..first + count {
                if block >= new_blocks || block < fs.first_data_block {
                    continue;
                }
                let owner = (block - fs.first_data_block) / fs.per_group;
                let owner_flags = le16(&descriptor(&table, owner), 0x12).unwrap_or(0);
                if owner_flags & EXT4_GROUP_BLOCK_UNINIT != 0 {
                    continue;
                }
                let index = bitmap_of(io, &mut changed_bitmaps, owner, &table)?;
                let bit = (block - fs.group_start(owner)) as usize;
                let bytes = &mut changed_bitmaps[index].1;
                if bytes[bit / 8] & (1 << (bit % 8)) != 0 {
                    bytes[bit / 8] &= !(1 << (bit % 8));
                    let at = owner as usize * fs.descriptor_bytes;
                    let slot = &mut table[at..at + fs.descriptor_bytes];
                    let free = fs.wide_field(slot, 0x0c, 0x2c, 16) + 1;
                    fs.set_free_blocks(slot, free);
                    freed_in_kept += 1;
                }
            }
        }
    }
    let last = kept - 1;
    let in_old = fs.in_group(last, fs.blocks);
    let in_new = fs.in_group(last, new_blocks);
    if in_new < in_old {
        let index = bitmap_of(io, &mut changed_bitmaps, last, &table)?;
        let bytes = &mut changed_bitmaps[index].1;
        for bit in in_new as usize..fs.per_group as usize {
            bytes[bit / 8] |= 1 << (bit % 8);
        }
        let at = last as usize * fs.descriptor_bytes;
        let slot = &mut table[at..at + fs.descriptor_bytes];
        let free = fs.wide_field(slot, 0x0c, 0x2c, 16).saturating_sub(in_old - in_new);
        fs.set_free_blocks(slot, free);
    }
    for (group, bytes) in &changed_bitmaps {
        let at = *group as usize * fs.descriptor_bytes;
        let bitmap_block = fs.metadata_blocks(&table[at..at + fs.descriptor_bytes])[0].0;
        if !io.write(bitmap_block * fs.block_bytes, bytes) {
            return Err(ShrinkError::Write);
        }
        let slot = &mut table[at..at + fs.descriptor_bytes];
        fs.bitmap_checksum(slot, bytes);
        fs.descriptor_checksum(*group, slot);
    }
    if !io.write(fs.table_at(), &table[..kept as usize * fs.descriptor_bytes]) {
        return Err(ShrinkError::Write);
    }
    let removed = old_groups - kept;
    let mut superblock = fs.superblock.clone();
    let read_wide = |sb: &[u8], low: usize, high: usize| {
        u64::from(le32(sb, low).unwrap_or(0))
            | match fs.wide {
                true => u64::from(le32(sb, high).unwrap_or(0)) << 32,
                false => 0,
            }
    };
    let write_wide = |sb: &mut [u8], low: usize, high: usize, value: u64| {
        put32(sb, low, value as u32);
        if fs.wide {
            put32(sb, high, (value >> 32) as u32);
        }
    };
    let cut_free = in_old.saturating_sub(in_new);
    let free_blocks = read_wide(&superblock, 0x0c, 0x158)
        .saturating_sub(freed_in_removed + cut_free)
        .saturating_add(freed_in_kept);
    let reserved = read_wide(&superblock, 0x08, 0x154);
    write_wide(&mut superblock, 0x04, 0x150, new_blocks);
    write_wide(&mut superblock, 0x0c, 0x158, free_blocks);
    write_wide(&mut superblock, 0x08, 0x154, reserved.min(new_blocks / 2));
    let inodes = u64::from(le32(&superblock, 0x00).unwrap_or(0));
    let free_inodes = u64::from(le32(&superblock, 0x10).unwrap_or(0));
    let gone = removed * fs.inodes_per_group;
    put32(&mut superblock, 0x00, inodes.saturating_sub(gone) as u32);
    put32(&mut superblock, 0x10, free_inodes.saturating_sub(gone) as u32);
    if fs.metadata_checksums {
        let crc = crc32c_raw(!0, &superblock[..EXT4_SUPERBLOCK_CHECKSUM_AT]);
        put32(&mut superblock, EXT4_SUPERBLOCK_CHECKSUM_AT, crc);
    }
    if !io.write(EXT4_SUPERBLOCK_AT, &superblock) {
        return Err(ShrinkError::Write);
    }
    Ok(new_blocks * fs.block_bytes)
}

const NTFS_SECTOR: usize = 512;
const NTFS_VOLUME_RECORD: u64 = 3;
const NTFS_VOLUME_INFORMATION: u32 = 0x70;
const NTFS_VOLUME_DIRTY: u16 = 1;
const NTFS_MOST_RECORD_BYTES: usize = 4096;
const NTFS_TOTAL_SECTORS_AT: usize = 0x28;

fn protect(record: &mut [u8]) -> Option<()> {
    let sequence_at = usize::from(le16(record, 4)?);
    let count = usize::from(le16(record, 6)?);
    let number = match le16(record, sequence_at)?.wrapping_add(1) {
        0 | 0xffff => 1,
        number => number,
    };
    put16(record, sequence_at, number);
    for sector in 1..count {
        let end = sector * NTFS_SECTOR - 2;
        let original = le16(record, end)?;
        put16(record, sequence_at + sector * 2, original);
        put16(record, end, number);
    }
    Some(())
}

/// Shrinks the NTFS volume `io` holds to at most `new_bytes`: the bytes it now takes, its
/// backup boot sector in the last of them.
pub fn shrink_ntfs(io: &mut dyn PartitionIo, new_bytes: u64) -> Result<u64, ShrinkError> {
    let mut boot = [0u8; NTFS_SECTOR];
    if !io.read(0, &mut boot) || &boot[3..11] != b"NTFS    " {
        return Err(ShrinkError::Read);
    }
    let fields = || -> Option<(u64, u64, u64, u64, u64, usize)> {
        let sector_bytes = u64::from(le16(&boot, 0x0b)?);
        let raw = boot[0x0d];
        let cluster_bytes = match raw {
            0 => return None,
            raw if raw > 0x80 => sector_bytes.checked_shl(256 - u32::from(raw))?,
            raw => sector_bytes * u64::from(raw),
        };
        let total = le64(&boot, NTFS_TOTAL_SECTORS_AT)?;
        let mft = le64(&boot, 0x30)?.checked_mul(cluster_bytes)?;
        let mirror = le64(&boot, 0x38)?.checked_mul(cluster_bytes)?;
        let raw_record = boot[0x40] as i8;
        let record_bytes = match raw_record {
            raw if raw > 0 => u64::from(raw as u8) * cluster_bytes,
            raw => 1u64.checked_shl(u32::from(raw.unsigned_abs()))?,
        } as usize;
        Some((sector_bytes, cluster_bytes, total, mft, mirror, record_bytes))
    };
    let (sector_bytes, cluster_bytes, total, mft, mirror, record_bytes) =
        fields().ok_or(ShrinkError::NotShrinkable)?;
    let record_fits = (NTFS_SECTOR..=NTFS_MOST_RECORD_BYTES).contains(&record_bytes);
    if sector_bytes != NTFS_SECTOR as u64 || !record_fits {
        return Err(ShrinkError::NotShrinkable);
    }
    let per_cluster = cluster_bytes / sector_bytes;
    let new_total = (new_bytes / sector_bytes).saturating_sub(1) / per_cluster * per_cluster;
    if new_total >= total {
        return Ok((total + 1) * sector_bytes);
    }
    let partition_bytes = (total + 1) * sector_bytes;
    let mut read = |at: u64, buffer: &mut [u8]| io.read(at, buffer);
    let tail = match crate::partitions::ntfs_tail(&boot, partition_bytes, &mut read) {
        Some(Ok(tail)) => tail,
        Some(Err(_)) => return Err(ShrinkError::NotShrinkable),
        None => return Err(ShrinkError::Read),
    };
    if (total - new_total) * sector_bytes > tail + cluster_bytes {
        return Err(ShrinkError::TailInUse);
    }
    let mut record = [0u8; NTFS_MOST_RECORD_BYTES];
    let record = &mut record[..record_bytes];
    let at = mft + NTFS_VOLUME_RECORD * record_bytes as u64;
    let mut read = |at: u64, buffer: &mut [u8]| io.read(at, buffer);
    ntfs_record(record, at, &mut read).ok_or(ShrinkError::Read)?;
    let attribute_at = {
        let attribute =
            ntfs_attribute(record, NTFS_VOLUME_INFORMATION).ok_or(ShrinkError::Read)?;
        attribute.as_ptr() as usize - record.as_ptr() as usize
    };
    let content = usize::from(le16(record, attribute_at + 0x14).ok_or(ShrinkError::Read)?);
    let flags_at = attribute_at + content + 0x0a;
    let flags = le16(record, flags_at).ok_or(ShrinkError::Read)?;
    put16(record, flags_at, flags | NTFS_VOLUME_DIRTY);
    protect(record).ok_or(ShrinkError::Read)?;
    let mirror_at = mirror + NTFS_VOLUME_RECORD * record_bytes as u64;
    if !io.write(at, record) || !io.write(mirror_at, record) {
        return Err(ShrinkError::Write);
    }
    let total_field = NTFS_TOTAL_SECTORS_AT..NTFS_TOTAL_SECTORS_AT + 8;
    boot[total_field].copy_from_slice(&new_total.to_le_bytes());
    if !io.write(new_total * sector_bytes, &boot) || !io.write(0, &boot) {
        return Err(ShrinkError::Write);
    }
    Ok((new_total + 1) * sector_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Memory(Vec<u8>);

    impl PartitionIo for Memory {
        fn read(&mut self, at: u64, buffer: &mut [u8]) -> bool {
            let at = at as usize;
            match self.0.get(at..at + buffer.len()) {
                Some(bytes) => {
                    buffer.copy_from_slice(bytes);
                    true
                }
                None => false,
            }
        }

        fn write(&mut self, at: u64, bytes: &[u8]) -> bool {
            let at = at as usize;
            match self.0.get_mut(at..at + bytes.len()) {
                Some(slot) => {
                    slot.copy_from_slice(bytes);
                    true
                }
                None => false,
            }
        }
    }

    fn ext4_volume(groups: u64, used_in_first: u64) -> Memory {
        let block = 1024usize;
        let per_group = 1024u64;
        let blocks = 1 + groups * per_group;
        let mut disk = alloc::vec![0u8; blocks as usize * block];
        let sb = &mut disk[1024..2048];
        put32(sb, 0x00, (groups * 16) as u32);
        put32(sb, 0x04, blocks as u32);
        put32(sb, 0x0c, (blocks - 1 - used_in_first) as u32);
        put32(sb, 0x10, (groups * 16 - 11) as u32);
        put32(sb, 0x14, 1);
        put32(sb, 0x18, 0);
        put32(sb, 0x20, per_group as u32);
        put32(sb, 0x28, 16);
        put16(sb, 0x38, 0xef53);
        put16(sb, 0x3a, EXT4_STATE_CLEAN);
        put16(sb, 0x58, 128);
        for group in 0..groups {
            let at = 2048 + group as usize * EXT4_SMALL_DESCRIPTOR;
            let bitmap_block = 3 + group * 4;
            let descriptor = &mut disk[at..at + EXT4_SMALL_DESCRIPTOR];
            put32(descriptor, 0x00, bitmap_block as u32);
            put32(descriptor, 0x04, (bitmap_block + 1) as u32);
            put32(descriptor, 0x08, (bitmap_block + 2) as u32);
            let used = if group == 0 { used_in_first } else { 0 };
            put16(descriptor, 0x0c, (per_group - used) as u16);
            put16(descriptor, 0x0e, if group == 0 { 5 } else { 16 });
            let bitmap_at = bitmap_block as usize * block;
            for bit in 0..used as usize {
                disk[bitmap_at + bit / 8] |= 1 << (bit % 8);
            }
        }
        Memory(disk)
    }

    #[test]
    fn ext4_loses_its_free_groups_and_pads_its_new_last_group() {
        let mut volume = ext4_volume(4, 200);
        let new_bytes = (1 + 1024 + 512) * 1024;
        let shrunk = shrink_ext4(&mut volume, new_bytes).unwrap();
        assert_eq!(shrunk, new_bytes);
        let sb = &volume.0[1024..2048];
        assert_eq!(le32(sb, 0x04), Some(1 + 1024 + 512));
        assert_eq!(le32(sb, 0x00), Some(32));
        let bitmap_at = (3 + 4) * 1024;
        assert_eq!(volume.0[bitmap_at + 512 / 8], 0xff);
        assert_eq!(volume.0[bitmap_at], 0);
        let descriptor = &volume.0[2048 + 32..2048 + 64];
        assert_eq!(le16(descriptor, 0x0c), Some(512));
    }

    #[test]
    fn ext4_refuses_when_a_used_block_lies_beyond_the_new_end() {
        let mut volume = ext4_volume(2, 1000);
        assert_eq!(
            shrink_ext4(&mut volume, 500 * 1024),
            Err(ShrinkError::TailInUse)
        );
    }
}
