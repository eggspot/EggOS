use crate::checksum::Crc64;
use crate::layout::{
    BITS_PER_BLOCK, BLOCK_SIZE, Block, DESCRIPTOR_MAGIC, Descriptor, FIRST_FREE_INODE,
    FORMATTING_MAGIC, Geometry, Inode, InodeKind, MAX_LABEL, ROOT_INODE, commit_matches,
    encode_commit, u64_at, zeroed_block,
};
use crate::volume::Volume;
use crate::{Disk, DiskError, Error, MountError, Mounted};
use alloc::vec::Vec;

const BLANK_BLOCKS: u64 = 32;

const WRITE_RUN_BLOCKS: usize = 16;

/// What a disk holds, as [`probe`] finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contents {
    /// An EggFS file system.
    EggFs,
    /// Nothing: its first 128 KiB are zeros, or an EggFS format stopped before it ended.
    Blank,
    /// Something else, which must not be formatted unasked.
    Other,
}

/// What `disk` holds.
pub fn probe<D: Disk + ?Sized>(disk: &mut D) -> Result<Contents, DiskError> {
    if disk.blocks() == 0 {
        return Ok(Contents::Other);
    }
    let mut block = zeroed_block();
    disk.read(0, &mut block[..])?;
    if Geometry::decode(&block[..]).is_some() {
        return Ok(Contents::EggFs);
    }
    if block[..8] == FORMATTING_MAGIC {
        return Ok(Contents::Blank);
    }
    for index in 0..BLANK_BLOCKS.min(disk.blocks()) {
        if index > 0 {
            disk.read(index, &mut block[..])?;
        }
        if block.iter().any(|&byte| byte != 0) {
            return Ok(Contents::Other);
        }
    }
    Ok(Contents::Blank)
}

/// Why [`format`] did not format a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// The disk is smaller than 1 MiB.
    TooSmall,
    /// The disk failed a write.
    Io,
    /// The size asked for is larger than the disk.
    LargerThanDisk,
    /// The label is longer than [`crate::MAX_LABEL`] bytes or holds NUL.
    BadLabel,
}

/// How [`format_with`] makes a file system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatOptions<'a> {
    /// The blocks of [`crate::BLOCK_SIZE`] bytes to use from the start of the disk; all of it
    /// when `None`.
    pub blocks: Option<u64>,
    /// The volume's name, empty for none.
    pub label: &'a str,
}

fn bitmap_block(index: u32, used_below: u32, total: u32) -> alloc::boxed::Box<Block> {
    let mut block = zeroed_block();
    let first = index * BITS_PER_BLOCK;
    for bit in 0..BITS_PER_BLOCK {
        let number = first + bit;
        if number < used_below || number >= total {
            block[(bit / 8) as usize] |= 1 << (bit % 8);
        }
    }
    block
}

/// Writes an empty EggFS file system on all of `disk` (up to 16 TiB), its root directory
/// stamped `now`. The superblock goes last, after a marker that [`probe`] reads as blank, so a
/// format that stops midway is done again rather than mounted.
pub fn format<D: Disk + ?Sized>(disk: &mut D, now: u64) -> Result<(), FormatError> {
    format_with(disk, now, &FormatOptions::default())
}

/// Writes an empty EggFS file system as [`format`] does, on the part of `disk` and with the
/// label `options` ask for.
pub fn format_with<D: Disk + ?Sized>(
    disk: &mut D,
    now: u64,
    options: &FormatOptions<'_>,
) -> Result<(), FormatError> {
    let blocks = options.blocks.unwrap_or(disk.blocks());
    if blocks > disk.blocks() {
        return Err(FormatError::LargerThanDisk);
    }
    let label = options.label.as_bytes();
    if label.len() > MAX_LABEL || label.contains(&0) {
        return Err(FormatError::BadLabel);
    }
    let mut geometry = Geometry::for_disk(blocks, now).ok_or(FormatError::TooSmall)?;
    geometry.label[..label.len()].copy_from_slice(label);
    let io = |_: DiskError| FormatError::Io;
    let mut marker = zeroed_block();
    marker[..8].copy_from_slice(&FORMATTING_MAGIC);
    disk.write(0, &marker[..]).map_err(io)?;
    disk.flush().map_err(io)?;
    for index in 0..geometry.block_bitmap_blocks {
        let block = bitmap_block(index, geometry.data_start, geometry.blocks);
        disk.write(u64::from(geometry.block_bitmap + index), &block[..])
            .map_err(io)?;
    }
    for index in 0..geometry.inode_bitmap_blocks {
        let block = bitmap_block(index, FIRST_FREE_INODE, geometry.inodes);
        disk.write(u64::from(geometry.inode_bitmap + index), &block[..])
            .map_err(io)?;
    }
    disk.write(u64::from(geometry.journal), &zeroed_block()[..])
        .map_err(io)?;
    let mut table = zeroed_block();
    let root = Inode::new(InodeKind::Directory, 0o755, ROOT_INODE, 1, now);
    let (block, at) = geometry.inode_location(ROOT_INODE);
    root.encode(ROOT_INODE, &mut table[at..]);
    disk.write(u64::from(block), &table[..]).map_err(io)?;
    disk.flush().map_err(io)?;
    disk.write(0, &geometry.encode()[..]).map_err(io)?;
    disk.flush().map_err(io)
}

fn is_replayable(geometry: &Geometry, target: u32) -> bool {
    let journal_end = geometry.journal + geometry.journal_blocks;
    target < geometry.blocks && !(geometry.journal..journal_end).contains(&target)
}

fn is_same_superblock(geometry: &Geometry, image: &[u8]) -> bool {
    Geometry::decode(image).is_some_and(|upgraded| geometry.same_layout(&upgraded))
}

pub(crate) fn replay<D: Disk>(disk: &mut D, geometry: &Geometry) -> Result<(u64, u32), MountError> {
    let io = |_: DiskError| MountError::Io;
    let journal = u64::from(geometry.journal);
    let mut block = zeroed_block();
    disk.read(journal, &mut block[..]).map_err(io)?;
    let Some(descriptor) = Descriptor::decode(&block[..]) else {
        let next = match block[..8] == DESCRIPTOR_MAGIC {
            true => u64_at(&block[..], 8).wrapping_add(2),
            false => 1,
        };
        return Ok((next, 0));
    };
    let next = descriptor.sequence.wrapping_add(1);
    let count = descriptor.targets.len() as u32;
    if count > geometry.transaction_capacity() {
        return Ok((next, 0));
    }
    let mut crc = Crc64::new();
    crc.update(&block[..]);
    let mut image = zeroed_block();
    for index in 0..u64::from(count) {
        disk.read(journal + 1 + index, &mut image[..]).map_err(io)?;
        crc.update(&image[..]);
    }
    disk.read(journal + 1 + u64::from(count), &mut block[..])
        .map_err(io)?;
    if !commit_matches(&block[..], descriptor.sequence, count, crc.finish()) {
        return Ok((next, 0));
    }
    if !descriptor
        .targets
        .iter()
        .all(|&target| is_replayable(geometry, target))
    {
        return Err(MountError::Corrupt);
    }
    for (index, &target) in descriptor.targets.iter().enumerate() {
        if target == 0 {
            disk.read(journal + 1 + index as u64, &mut image[..])
                .map_err(io)?;
            if !is_same_superblock(geometry, &image[..]) {
                return Err(MountError::Corrupt);
            }
        }
    }
    let mut recovered = 0;
    for (index, &target) in descriptor.targets.iter().enumerate() {
        disk.read(journal + 1 + index as u64, &mut image[..])
            .map_err(io)?;
        disk.read(u64::from(target), &mut block[..]).map_err(io)?;
        if block[..] != image[..] {
            disk.write(u64::from(target), &image[..]).map_err(io)?;
            recovered += 1;
        }
    }
    if recovered > 0 {
        disk.flush().map_err(io)?;
    }
    Ok((next, recovered))
}

pub(crate) fn count_clear<D: Disk>(
    disk: &mut D,
    start: u32,
    blocks: u32,
    bits: u32,
) -> Result<u64, DiskError> {
    let mut block = zeroed_block();
    let mut clear = 0u64;
    for index in 0..blocks {
        disk.read(u64::from(start + index), &mut block[..])?;
        let first = index * BITS_PER_BLOCK;
        let valid = bits.saturating_sub(first).min(BITS_PER_BLOCK);
        let whole_bytes = (valid / 8) as usize;
        clear += block[..whole_bytes]
            .iter()
            .map(|byte| u64::from(byte.count_zeros()))
            .sum::<u64>();
        for bit in whole_bytes as u32 * 8..valid {
            if block[(bit / 8) as usize] & (1 << (bit % 8)) == 0 {
                clear += 1;
            }
        }
    }
    Ok(clear)
}

impl<D: Disk> Volume<D> {
    /// Mounts the EggFS file system on `disk`, first restoring from the journal a committed
    /// transaction whose blocks did not all reach their places, then freeing, in a commit of
    /// its own, the files removed while open that a crash or a power-off left behind. On an
    /// error the disk comes back.
    pub fn mount(mut disk: D) -> Result<(Self, Mounted), (MountError, D)> {
        let mut block = zeroed_block();
        if disk.read(0, &mut block[..]).is_err() {
            return Err((MountError::Io, disk));
        }
        let Some(geometry) = Geometry::decode(&block[..]) else {
            return Err((MountError::NotEggFs, disk));
        };
        if u64::from(geometry.blocks) > disk.blocks() {
            return Err((MountError::Corrupt, disk));
        }
        let (sequence, recovered_blocks) = match replay(&mut disk, &geometry) {
            Ok(replayed) => replayed,
            Err(error) => return Err((error, disk)),
        };
        let geometry = match recovered_blocks {
            0 => geometry,
            _ => {
                if disk.read(0, &mut block[..]).is_err() {
                    return Err((MountError::Io, disk));
                }
                match Geometry::decode(&block[..]) {
                    Some(replayed) if geometry.same_layout(&replayed) => replayed,
                    _ => return Err((MountError::Corrupt, disk)),
                }
            }
        };
        let free = count_clear(
            &mut disk,
            geometry.block_bitmap,
            geometry.block_bitmap_blocks,
            geometry.blocks,
        )
        .and_then(|blocks| {
            count_clear(
                &mut disk,
                geometry.inode_bitmap,
                geometry.inode_bitmap_blocks,
                geometry.inodes,
            )
            .map(|inodes| (blocks, inodes))
        });
        let Ok((free_blocks, free_inodes)) = free else {
            return Err((MountError::Io, disk));
        };
        let mut volume = Volume::assemble(disk, geometry, sequence);
        volume.free_blocks = free_blocks;
        volume.free_inodes = free_inodes;
        let root = match volume.inode(ROOT_INODE) {
            Ok(root) if root.kind == InodeKind::Directory => root,
            Ok(_) | Err(Error::Corrupt) => return Err((MountError::Corrupt, volume.into_disk())),
            Err(_) => return Err((MountError::Io, volume.into_disk())),
        };
        volume.root_generation = root.generation;
        let freed_orphans = match volume.free_left_orphans() {
            Ok(freed) => freed,
            Err(Error::Io) => return Err((MountError::Io, volume.into_disk())),
            Err(_) => return Err((MountError::Corrupt, volume.into_disk())),
        };
        let mounted = Mounted {
            recovered_blocks,
            freed_orphans,
        };
        Ok((volume, mounted))
    }

    /// Commits the changes made since the last commit: journal, commit record, then the blocks
    /// in their places, flushing the disk after each (see the crate documentation). Nothing to
    /// commit costs nothing. A disk failure here leaves the volume refusing every change.
    pub fn commit(&mut self) -> Result<(), Error> {
        if self.broken {
            return Err(Error::Io);
        }
        let targets = self.cache.dirty_blocks();
        if targets.is_empty() {
            return Ok(());
        }
        if targets.len() > self.geometry.transaction_capacity() as usize {
            self.broken = true;
            return Err(Error::Io);
        }
        let descriptor = Descriptor {
            sequence: self.sequence,
            targets,
        };
        let header = descriptor.encode();
        let mut crc = Crc64::new();
        crc.update(&header[..]);
        for &target in &descriptor.targets {
            crc.update(&self.cache.data(target).ok_or(Error::Io)?[..]);
        }
        if self
            .write_transaction(&descriptor, &header[..], crc.finish())
            .is_err()
        {
            self.broken = true;
            return Err(Error::Io);
        }
        self.cache.mark_clean();
        self.freed_waiting = 0;
        self.sequence = self.sequence.wrapping_add(1);
        self.commits += 1;
        Ok(())
    }

    fn write_run(
        &mut self,
        at: u64,
        blocks: &[u32],
        buffer: &mut Vec<u8>,
    ) -> Result<(), DiskError> {
        buffer.clear();
        for &block in blocks {
            buffer.extend_from_slice(&self.cache.data(block).ok_or(DiskError)?[..]);
        }
        self.disk.write(at, buffer)
    }

    fn write_transaction(
        &mut self,
        descriptor: &Descriptor,
        header: &[u8],
        checksum: u64,
    ) -> Result<(), DiskError> {
        let journal = u64::from(self.geometry.journal);
        let targets = &descriptor.targets;
        let mut buffer = Vec::with_capacity(WRITE_RUN_BLOCKS * BLOCK_SIZE);
        self.disk.write(journal, header)?;
        for (index, run) in targets.chunks(WRITE_RUN_BLOCKS).enumerate() {
            let at = journal + 1 + (index * WRITE_RUN_BLOCKS) as u64;
            self.write_run(at, run, &mut buffer)?;
        }
        self.disk.flush()?;
        let count = targets.len() as u32;
        let commit = encode_commit(descriptor.sequence, count, checksum);
        self.disk
            .write(journal + 1 + u64::from(count), &commit[..])?;
        self.disk.flush()?;
        let mut start = 0;
        while start < targets.len() {
            let mut end = start + 1;
            while end < targets.len()
                && end - start < WRITE_RUN_BLOCKS
                && targets[end] == targets[end - 1] + 1
            {
                end += 1;
            }
            self.write_run(u64::from(targets[start]), &targets[start..end], &mut buffer)?;
            start = end;
        }
        self.disk.flush()
    }
}
