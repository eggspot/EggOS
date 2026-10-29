//! A read-only FAT file system on a disk (D130): FAT12, FAT16 and FAT32 volumes as Microsoft's
//! public FAT specification lays them out, on the whole disk or in the first partition of a
//! master boot record (as QEMU's `vvfat` presents a host folder), with long file names.
//!
//! - **Nodes.** A node is named by where its short directory entry is: the directory's first
//!   cluster (0 for the root directory) in the high 32 bits, the entry's index in that
//!   directory in the low ones; the root is [`ROOT`]. Nothing is kept per node, so a folder of
//!   any size costs nothing until it is read.
//! - **Names.** A long name (UTF-16, up to 255 units) is taken from the entries before its
//!   short entry, checked by ordinal and checksum; without one, or with one that does not fit
//!   [`super::NAME_MAX`] bytes as UTF-8, the short name is shown, in lower case where its
//!   entry says so. Lookups ignore ASCII case, as FAT does, and match either name.
//! - **Reads.** One sleeping lock over the disk (a thread that wants it while a transfer runs
//!   sleeps), a cache of [`CACHED_SECTORS`] sectors for directory
//!   entries, FAT entries and file ends, and cursors on cluster chains (one for file data, one
//!   for each of the last four directories) that also remember the two clusters behind them,
//!   so sequential reads, listings and lookups follow a chain once, long names that start in
//!   an earlier cluster included. Whole sectors of file data go from the disk straight to the
//!   caller, in runs of contiguous clusters. A chain that comes back to a cluster it passed is
//!   an I/O error, found within twice its length by keeping one cluster to compare with at
//!   doubling distances (Brent's method: a compare a hop, D155).
//! - **Read-only.** Every change is refused with `EROFS` (the trait's defaults). Permissions
//!   are 0755 (0555 for a file marked read-only), owner and group 0, times the last write time
//!   read as UTC; a directory reports one link and size 0, as its subdirectories are not
//!   counted.

use super::{Attributes, DirectoryEntry, FileSystem, NAME_MAX, Name, NodeKind, StorageError};
use crate::block::{BlockDevice, SECTOR_SIZE};
use crate::devices::Deadline;
use crate::sync::{SleepLock, Sleeping};
use alloc::boxed::Box;
use alloc::vec::Vec;

#[cfg(test)]
mod tests;

/// The node of the root directory.
pub const ROOT: u64 = u64::MAX;

/// Sectors of directory and FAT entries the file system keeps.
pub const CACHED_SECTORS: usize = 16;

/// How long one disk request may take before the read fails.
pub const REQUEST_TIMEOUT_MS: u64 = 2000;

const ENTRY_BYTES: u64 = 32;
const ROOT_FILE_ID: u64 = 1;
const FILE_ID_OFFSET: u64 = 2;
const MAX_DIRECTORY_ENTRIES: u32 = 65536;
const MAX_LONG_NAME_ENTRIES: u32 = 20;
const LONG_NAME_UNITS: usize = 13 * MAX_LONG_NAME_ENTRIES as usize;
const MAX_RUN_BYTES: u64 = 1 << 20;
const SMALLEST_CLUSTER_BYTES: usize = 512;
const DIRECTORY_CURSORS: usize = 4;
const CLUSTERS_BEHIND: usize =
    (MAX_LONG_NAME_ENTRIES as usize * ENTRY_BYTES as usize).div_ceil(SMALLEST_CLUSTER_BYTES);
const NO_DIRECTORY: u32 = u32::MAX;
const SHOWN_SHORT_NAME_BYTES: usize = 12;

mod attribute {
    pub const READ_ONLY: u8 = 0x01;
    pub const VOLUME_ID: u8 = 0x08;
    pub const DIRECTORY: u8 = 0x10;
    pub const LONG_NAME: u8 = 0x0f;
    pub const LONG_NAME_MASK: u8 = 0x3f;
}

const FREE_ENTRY: u8 = 0xe5;
const END_OF_DIRECTORY: u8 = 0x00;
const KANJI_E5: u8 = 0x05;
const LAST_LONG_ENTRY: u8 = 0x40;
const LOWER_CASE_BASE: u8 = 0x08;
const LOWER_CASE_EXTENSION: u8 = 0x10;

/// The width of a volume's FAT entries, which its cluster count decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

impl FatType {
    /// Its usual name.
    pub const fn name(self) -> &'static str {
        match self {
            FatType::Fat12 => "FAT12",
            FatType::Fat16 => "FAT16",
            FatType::Fat32 => "FAT32",
        }
    }

    const fn end_of_chain(self) -> u32 {
        match self {
            FatType::Fat12 => 0xff8,
            FatType::Fat16 => 0xfff8,
            FatType::Fat32 => 0x0fff_fff8,
        }
    }
}

/// Why a disk was not mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    /// Neither the disk nor its first partition holds a FAT volume.
    NotFat,
    /// The disk could not be read.
    Unreadable,
}

impl MountError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            MountError::NotFat => "no FAT file system on the disk or its first partition",
            MountError::Unreadable => "the disk could not be read",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootDirectory {
    Fixed { start: u64, entries: u32 },
    Clusters(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    fat_type: FatType,
    fat_start: u64,
    root: RootDirectory,
    data_start: u64,
    cluster_bytes: u64,
    clusters: u32,
    label: [u8; 11],
}

impl Layout {
    fn is_cluster(&self, cluster: u32) -> bool {
        cluster >= 2 && cluster - 2 < self.clusters
    }

    fn cluster_start(&self, cluster: u32) -> u64 {
        self.data_start + u64::from(cluster - 2) * self.cluster_bytes
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

const BOOT_SIGNATURE: [u8; 2] = [0x55, 0xaa];
const PARTITION_TABLE: usize = 446;
const PARTITION_ENTRY_BYTES: usize = 16;
const FAT32_NO_MIRRORING: u16 = 0x80;
const FAT32_ACTIVE_FAT_MASK: u16 = 0x0f;

fn parse_boot_sector(sector: &[u8; SECTOR_SIZE], start: u64, disk_bytes: u64) -> Option<Layout> {
    if !matches!(sector[0], 0xeb | 0xe9) || sector[510..512] != BOOT_SIGNATURE {
        return None;
    }
    let bytes_per_sector = u64::from(u16_at(sector, 11));
    let sectors_per_cluster = u64::from(sector[13]);
    let reserved = u64::from(u16_at(sector, 14));
    let fats = u64::from(sector[16]);
    let root_entries = u64::from(u16_at(sector, 17));
    let total = match u16_at(sector, 19) {
        0 => u64::from(u32_at(sector, 32)),
        total => u64::from(total),
    };
    let fat_size = match u16_at(sector, 22) {
        0 => u64::from(u32_at(sector, 36)),
        size => u64::from(size),
    };
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096)
        || !sectors_per_cluster.is_power_of_two()
        || reserved == 0
        || fats == 0
        || fat_size == 0
    {
        return None;
    }
    let root_sectors = (root_entries * ENTRY_BYTES).div_ceil(bytes_per_sector);
    let metadata = reserved + fats * fat_size + root_sectors;
    let clusters = total.checked_sub(metadata)? / sectors_per_cluster;
    let fat_type = match clusters {
        0 => return None,
        1..4085 => FatType::Fat12,
        4085..65525 => FatType::Fat16,
        _ => FatType::Fat32,
    };
    let clusters = u32::try_from(clusters).ok().filter(|&n| n <= 0x0fff_fff5)?;
    let fat_bytes_needed = match fat_type {
        FatType::Fat12 => (u64::from(clusters) + 2) * 3 / 2 + 1,
        FatType::Fat16 => (u64::from(clusters) + 2) * 2,
        FatType::Fat32 => (u64::from(clusters) + 2) * 4,
    };
    if fat_size * bytes_per_sector < fat_bytes_needed
        || start.checked_add(total * bytes_per_sector)? > disk_bytes
    {
        return None;
    }
    let (root, label_at) = match fat_type {
        FatType::Fat32 => {
            if root_entries != 0 {
                return None;
            }
            (RootDirectory::Clusters(u32_at(sector, 44)), 71)
        }
        _ => {
            if root_entries == 0 {
                return None;
            }
            let root_start = start + (reserved + fats * fat_size) * bytes_per_sector;
            let entries = root_entries as u32;
            (
                RootDirectory::Fixed {
                    start: root_start,
                    entries,
                },
                43,
            )
        }
    };
    let mirroring_off = fat_type == FatType::Fat32 && u16_at(sector, 40) & FAT32_NO_MIRRORING != 0;
    let active_fat = match mirroring_off {
        true => u64::from(u16_at(sector, 40) & FAT32_ACTIVE_FAT_MASK),
        false => 0,
    };
    if active_fat >= fats {
        return None;
    }
    let layout = Layout {
        fat_type,
        fat_start: start + (reserved + active_fat * fat_size) * bytes_per_sector,
        root,
        data_start: start + metadata * bytes_per_sector,
        cluster_bytes: sectors_per_cluster * bytes_per_sector,
        clusters,
        label: sector[label_at..label_at + 11].try_into().ok()?,
    };
    if let RootDirectory::Clusters(cluster) = root
        && !layout.is_cluster(cluster)
    {
        return None;
    }
    Some(layout)
}

fn partition_starts(sector: &[u8; SECTOR_SIZE]) -> impl Iterator<Item = u64> + '_ {
    let signed = sector[510..512] == BOOT_SIGNATURE;
    (0..4)
        .filter(move |_| signed)
        .map(|index| &sector[PARTITION_TABLE + index * PARTITION_ENTRY_BYTES..])
        .filter(|entry| entry[4] != 0 && u32_at(entry, 12) != 0)
        .map(|entry| u64::from(u32_at(entry, 8)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directory {
    Fixed { start: u64, entries: u32 },
    Clusters(u32),
    Empty,
}

#[derive(Debug, Clone, Copy, Default)]
struct ChainCursor {
    first: u32,
    index: u64,
    cluster: u32,
    behind: [u32; CLUSTERS_BEHIND],
    mark: u32,
    since_mark: u32,
    next_mark: u32,
}

#[derive(Debug, Clone, Copy)]
struct ListingCursor {
    directory: u32,
    position: u64,
    index: u32,
}

#[derive(Clone, Copy)]
struct ShortEntry([u8; ENTRY_BYTES as usize]);

impl ShortEntry {
    fn attributes(&self) -> u8 {
        self.0[11]
    }

    fn is_directory(&self) -> bool {
        self.attributes() & attribute::DIRECTORY != 0
    }

    fn first_cluster(&self, fat_type: FatType) -> u32 {
        let high = match fat_type {
            FatType::Fat32 => u32::from(u16_at(&self.0, 20)) << 16,
            _ => 0,
        };
        high | u32::from(u16_at(&self.0, 26))
    }

    fn size(&self) -> u64 {
        u64::from(u32_at(&self.0, 28))
    }

    fn checksum(&self) -> u8 {
        self.0[..11]
            .iter()
            .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte))
    }

    fn modified(&self) -> u64 {
        unix_time(u16_at(&self.0, 24), u16_at(&self.0, 22))
    }

    fn shown_name(&self) -> ([u8; SHOWN_SHORT_NAME_BYTES], usize) {
        let flags = self.0[12];
        let mut name = [0u8; SHOWN_SHORT_NAME_BYTES];
        let mut length = 0;
        let mut push = |byte: u8, lower: bool| {
            let byte = match byte {
                b' ' => return,
                0x80.. => b'_',
                _ if lower => byte.to_ascii_lowercase(),
                _ => byte,
            };
            name[length] = byte;
            length += 1;
        };
        for (index, &byte) in self.0[..8].iter().enumerate() {
            let byte = if index == 0 && byte == KANJI_E5 {
                FREE_ENTRY
            } else {
                byte
            };
            push(byte, flags & LOWER_CASE_BASE != 0);
        }
        if self.0[8..11].iter().any(|&byte| byte != b' ') {
            push(b'.', false);
            for &byte in &self.0[8..11] {
                push(byte, flags & LOWER_CASE_EXTENSION != 0);
            }
        }
        (name, length)
    }
}

enum Slot {
    End,
    Skip,
    LongName,
    Short(ShortEntry),
}

fn classify(raw: [u8; ENTRY_BYTES as usize]) -> Slot {
    let attributes = raw[11];
    match raw[0] {
        END_OF_DIRECTORY => Slot::End,
        FREE_ENTRY => Slot::Skip,
        _ if attributes & attribute::LONG_NAME_MASK == attribute::LONG_NAME => Slot::LongName,
        _ if attributes & attribute::VOLUME_ID != 0 => Slot::Skip,
        b'.' if raw[1..11].iter().all(|&byte| byte == b' ' || byte == b'.') => Slot::Skip,
        _ => Slot::Short(ShortEntry(raw)),
    }
}

fn unix_time(date: u16, time: u16) -> u64 {
    let year = 1980 + i64::from(date >> 9);
    let month = i64::from((date >> 5) & 0xf);
    let day = i64::from(date & 0x1f);
    if !(1..=12).contains(&month) || day == 0 {
        return 0;
    }
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = i64::from(time >> 11) * 3600
        + i64::from((time >> 5) & 0x3f) * 60
        + i64::from(time & 0x1f) * 2;
    (days * 86_400 + seconds).max(0) as u64
}

struct SectorCache {
    sectors: [u64; CACHED_SECTORS],
    last_used: [u32; CACHED_SECTORS],
    clock: u32,
    data: Box<[[u8; SECTOR_SIZE]]>,
}

impl SectorCache {
    fn new() -> Self {
        Self {
            sectors: [u64::MAX; CACHED_SECTORS],
            last_used: [0; CACHED_SECTORS],
            clock: 0,
            data: alloc::vec![[0; SECTOR_SIZE]; CACHED_SECTORS].into_boxed_slice(),
        }
    }
}

struct Volume {
    disk: Box<dyn BlockDevice>,
    new_deadline: fn(u64) -> Deadline,
    cache: SectorCache,
    data_cursor: ChainCursor,
    directory_cursors: [ChainCursor; DIRECTORY_CURSORS],
    next_directory_cursor: usize,
    listing: Option<ListingCursor>,
}

impl Volume {
    fn deadline(&self) -> Deadline {
        (self.new_deadline)(REQUEST_TIMEOUT_MS)
    }

    fn sector(&mut self, sector: u64) -> Result<&[u8; SECTOR_SIZE], StorageError> {
        let cache = &mut self.cache;
        cache.clock = cache.clock.wrapping_add(1);
        let slot = match cache.sectors.iter().position(|&cached| cached == sector) {
            Some(slot) => slot,
            None => {
                let slot = (0..CACHED_SECTORS)
                    .max_by_key(|&slot| cache.clock.wrapping_sub(cache.last_used[slot]))
                    .unwrap_or(0);
                cache.sectors[slot] = u64::MAX;
                let mut deadline = (self.new_deadline)(REQUEST_TIMEOUT_MS);
                self.disk
                    .read(sector, &mut cache.data[slot], &mut deadline)
                    .map_err(|_| StorageError)?;
                cache.sectors[slot] = sector;
                slot
            }
        };
        cache.last_used[slot] = cache.clock;
        Ok(&cache.data[slot])
    }

    fn read_small(&mut self, at: u64, out: &mut [u8]) -> Result<(), StorageError> {
        let mut done = 0;
        while done < out.len() {
            let position = at + done as u64;
            let within = (position % SECTOR_SIZE as u64) as usize;
            let sector = self.sector(position / SECTOR_SIZE as u64)?;
            let length = (SECTOR_SIZE - within).min(out.len() - done);
            out[done..done + length].copy_from_slice(&sector[within..within + length]);
            done += length;
        }
        Ok(())
    }

    fn read_data(&mut self, at: u64, out: &mut [u8]) -> Result<usize, StorageError> {
        let whole = out.len() / SECTOR_SIZE * SECTOR_SIZE;
        if !at.is_multiple_of(SECTOR_SIZE as u64) || whole == 0 {
            let within = (at % SECTOR_SIZE as u64) as usize;
            let length = (SECTOR_SIZE - within).min(out.len());
            self.read_small(at, &mut out[..length])?;
            return Ok(length);
        }
        let mut deadline = self.deadline();
        self.disk
            .read(at / SECTOR_SIZE as u64, &mut out[..whole], &mut deadline)
            .map_err(|_| StorageError)?;
        Ok(whole)
    }

    fn next_cluster(&mut self, layout: &Layout, cluster: u32) -> Result<Option<u32>, StorageError> {
        let value = match layout.fat_type {
            FatType::Fat12 => {
                let mut bytes = [0u8; 2];
                let at = u64::from(cluster) + u64::from(cluster / 2);
                self.read_small(layout.fat_start + at, &mut bytes)?;
                let pair = u16::from_le_bytes(bytes);
                u32::from(if cluster % 2 == 1 {
                    pair >> 4
                } else {
                    pair & 0xfff
                })
            }
            FatType::Fat16 => {
                let mut bytes = [0u8; 2];
                self.read_small(layout.fat_start + 2 * u64::from(cluster), &mut bytes)?;
                u32::from(u16::from_le_bytes(bytes))
            }
            FatType::Fat32 => {
                let mut bytes = [0u8; 4];
                self.read_small(layout.fat_start + 4 * u64::from(cluster), &mut bytes)?;
                u32::from_le_bytes(bytes) & 0x0fff_ffff
            }
        };
        Ok((value < layout.fat_type.end_of_chain() && layout.is_cluster(value)).then_some(value))
    }

    fn directory_cursor_slot(&mut self, first: u32) -> usize {
        match self
            .directory_cursors
            .iter()
            .position(|cursor| cursor.first == first)
        {
            Some(slot) => slot,
            None => {
                let slot = self.next_directory_cursor;
                self.next_directory_cursor = (slot + 1) % DIRECTORY_CURSORS;
                slot
            }
        }
    }

    fn cluster_in_chain(
        &mut self,
        layout: &Layout,
        directory_chain: bool,
        first: u32,
        index: u64,
    ) -> Result<Option<u32>, StorageError> {
        if !layout.is_cluster(first) || index >= u64::from(layout.clusters) {
            return Ok(None);
        }
        let slot = directory_chain.then(|| self.directory_cursor_slot(first));
        let cursor = match slot {
            Some(slot) => self.directory_cursors[slot],
            None => self.data_cursor,
        };
        if cursor.first == first && index < cursor.index {
            let back = (cursor.index - index) as usize;
            if let Some(&cluster) = cursor.behind.get(back - 1)
                && cluster != 0
            {
                return Ok(Some(cluster));
            }
        }
        let mut at = match cursor.first == first && cursor.index <= index {
            true => cursor,
            false => ChainCursor {
                first,
                index: 0,
                cluster: first,
                behind: [0; CLUSTERS_BEHIND],
                mark: first,
                since_mark: 0,
                next_mark: 1,
            },
        };
        while at.index < index {
            match self.next_cluster(layout, at.cluster)? {
                Some(next) if next == at.mark => return Err(StorageError),
                Some(next) => {
                    at.behind.rotate_right(1);
                    at.behind[0] = at.cluster;
                    at.cluster = next;
                    at.index += 1;
                    at.since_mark += 1;
                    if at.since_mark == at.next_mark {
                        at.mark = next;
                        at.next_mark = at.next_mark.saturating_mul(2);
                        at.since_mark = 0;
                    }
                }
                None => return Ok(None),
            }
        }
        match slot {
            Some(slot) => self.directory_cursors[slot] = at,
            None => self.data_cursor = at,
        }
        Ok(Some(at.cluster))
    }

    fn raw_entry(
        &mut self,
        layout: &Layout,
        directory: Directory,
        index: u32,
    ) -> Result<Option<[u8; ENTRY_BYTES as usize]>, StorageError> {
        let at = match directory {
            Directory::Empty => return Ok(None),
            Directory::Fixed { start, entries } => {
                if index >= entries {
                    return Ok(None);
                }
                start + u64::from(index) * ENTRY_BYTES
            }
            Directory::Clusters(first) => {
                if index >= MAX_DIRECTORY_ENTRIES {
                    return Ok(None);
                }
                let per_cluster = layout.cluster_bytes / ENTRY_BYTES;
                let offset = u64::from(index);
                let Some(cluster) =
                    self.cluster_in_chain(layout, true, first, offset / per_cluster)?
                else {
                    return Ok(None);
                };
                layout.cluster_start(cluster) + offset % per_cluster * ENTRY_BYTES
            }
        };
        let mut raw = [0u8; ENTRY_BYTES as usize];
        self.read_small(at, &mut raw)?;
        Ok(Some(raw))
    }

    fn next_short_entry(
        &mut self,
        layout: &Layout,
        directory: Directory,
        from: u32,
    ) -> Result<Option<(u32, ShortEntry)>, StorageError> {
        let mut index = from;
        while let Some(raw) = self.raw_entry(layout, directory, index)? {
            match classify(raw) {
                Slot::End => return Ok(None),
                Slot::Short(entry) => return Ok(Some((index, entry))),
                Slot::Skip | Slot::LongName => {}
            }
            index += 1;
        }
        Ok(None)
    }

    fn long_name(
        &mut self,
        layout: &Layout,
        directory: Directory,
        index: u32,
        entry: &ShortEntry,
        units: &mut [u16; LONG_NAME_UNITS],
    ) -> Result<Option<usize>, StorageError> {
        let checksum = entry.checksum();
        for ordinal in 1..=MAX_LONG_NAME_ENTRIES {
            let Some(at) = index.checked_sub(ordinal) else {
                return Ok(None);
            };
            let Some(raw) = self.raw_entry(layout, directory, at)? else {
                return Ok(None);
            };
            let is_part = raw[11] & attribute::LONG_NAME_MASK == attribute::LONG_NAME
                && raw[0] != FREE_ENTRY
                && u32::from(raw[0] & 0x1f) == ordinal
                && raw[13] == checksum;
            if !is_part {
                return Ok(None);
            }
            let start = (ordinal as usize - 1) * 13;
            let pieces = [(1, 5), (14, 6), (28, 2)];
            let mut unit = start;
            for (offset, count) in pieces {
                for at in 0..count {
                    units[unit] = u16_at(&raw, offset + 2 * at);
                    unit += 1;
                }
            }
            if raw[0] & LAST_LONG_ENTRY != 0 {
                let end = start + 13;
                let length = units[..end]
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(end);
                return Ok((length > 0).then_some(length));
            }
        }
        Ok(None)
    }

    fn name(
        &mut self,
        layout: &Layout,
        directory: Directory,
        index: u32,
        entry: &ShortEntry,
    ) -> Result<Name, StorageError> {
        let mut units = [0u16; LONG_NAME_UNITS];
        if let Some(length) = self.long_name(layout, directory, index, entry, &mut units)?
            && let Some(name) = utf8_name(&units[..length])
        {
            return Ok(name);
        }
        let (short, length) = entry.shown_name();
        Ok(Name::new(&short[..length]))
    }

    fn entry_of(
        &mut self,
        layout: &Layout,
        node: u64,
    ) -> Result<Option<(Directory, u32, ShortEntry)>, StorageError> {
        if node == ROOT {
            return Ok(None);
        }
        let directory = directory_from_key(layout, (node >> 32) as u32);
        let index = node as u32;
        Ok(match self.raw_entry(layout, directory, index)? {
            Some(raw) => match classify(raw) {
                Slot::Short(entry) => Some((directory, index, entry)),
                _ => None,
            },
            None => None,
        })
    }
}

fn utf8_name(units: &[u16]) -> Option<Name> {
    let mut bytes = [0u8; NAME_MAX];
    let mut length = 0;
    for decoded in core::char::decode_utf16(units.iter().copied()) {
        let character = decoded.unwrap_or(char::REPLACEMENT_CHARACTER);
        if character == '/' || character == '\0' {
            return None;
        }
        let width = character.len_utf8();
        if length + width > NAME_MAX {
            return None;
        }
        character.encode_utf8(&mut bytes[length..]);
        length += width;
    }
    Some(Name::new(&bytes[..length]))
}

fn directory_from_key(layout: &Layout, key: u32) -> Directory {
    match key {
        0 => match layout.root {
            RootDirectory::Fixed { start, entries } => Directory::Fixed { start, entries },
            RootDirectory::Clusters(first) => Directory::Clusters(first),
        },
        NO_DIRECTORY => Directory::Empty,
        first => Directory::Clusters(first),
    }
}

fn key_of(layout: &Layout, entry: &ShortEntry) -> u32 {
    let first = entry.first_cluster(layout.fat_type);
    match layout.root {
        RootDirectory::Clusters(root) if first == root => 0,
        _ if layout.is_cluster(first) => first,
        _ => NO_DIRECTORY,
    }
}

fn node_of(key: u32, index: u32) -> u64 {
    u64::from(key) << 32 | u64::from(index)
}

/// A FAT volume on a disk, mounted read-only.
pub struct Fat {
    layout: Layout,
    device: (u32, u32),
    volume: SleepLock<Volume>,
}

impl core::fmt::Debug for Fat {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Fat")
            .field("fat_type", &self.layout.fat_type)
            .field("clusters", &self.layout.clusters)
            .finish_non_exhaustive()
    }
}

impl Fat {
    /// The FAT volume on `disk`, on the whole disk or in its first partition that holds one,
    /// with device number `device`; each disk request polls until a deadline `new_deadline`
    /// makes, and threads waiting for the volume meanwhile wait as `sleeping` says. On an
    /// error the disk comes back.
    pub fn mount(
        mut disk: Box<dyn BlockDevice>,
        new_deadline: fn(u64) -> Deadline,
        sleeping: Sleeping,
        device: (u32, u32),
    ) -> Result<Self, (MountError, Box<dyn BlockDevice>)> {
        let disk_bytes = disk.sectors().saturating_mul(SECTOR_SIZE as u64);
        let mut sector = [0u8; SECTOR_SIZE];
        let read = |disk: &mut Box<dyn BlockDevice>, at: u64, sector: &mut [u8; SECTOR_SIZE]| {
            disk.read(at, sector, &mut new_deadline(REQUEST_TIMEOUT_MS))
        };
        if read(&mut disk, 0, &mut sector).is_err() {
            return Err((MountError::Unreadable, disk));
        }
        let mut layout = parse_boot_sector(&sector, 0, disk_bytes);
        if layout.is_none() {
            let starts: Vec<u64> = partition_starts(&sector).collect();
            for start in starts {
                if start >= disk.sectors() || read(&mut disk, start, &mut sector).is_err() {
                    continue;
                }
                layout = parse_boot_sector(&sector, start * SECTOR_SIZE as u64, disk_bytes);
                if layout.is_some() {
                    break;
                }
            }
        }
        let Some(layout) = layout else {
            return Err((MountError::NotFat, disk));
        };
        Ok(Self {
            layout,
            device,
            volume: SleepLock::new(
                Volume {
                    disk,
                    new_deadline,
                    cache: SectorCache::new(),
                    data_cursor: ChainCursor::default(),
                    directory_cursors: [ChainCursor::default(); DIRECTORY_CURSORS],
                    next_directory_cursor: 0,
                    listing: None,
                },
                sleeping,
            ),
        })
    }

    /// The width of its FAT entries.
    pub fn fat_type(&self) -> FatType {
        self.layout.fat_type
    }

    /// Its volume label from the boot sector, spaces at the end left out.
    pub fn label(&self) -> &[u8] {
        let end = self
            .layout
            .label
            .iter()
            .rposition(|&byte| byte != b' ' && byte != 0)
            .map_or(0, |at| at + 1);
        &self.layout.label[..end]
    }

    fn key_of_directory(&self, volume: &mut Volume, node: u64) -> Option<u32> {
        match volume.entry_of(&self.layout, node) {
            _ if node == ROOT => Some(0),
            Ok(Some((_, _, entry))) if entry.is_directory() => Some(key_of(&self.layout, &entry)),
            _ => None,
        }
    }
}

impl FileSystem for Fat {
    fn root(&self) -> u64 {
        ROOT
    }

    fn device(&self) -> (u32, u32) {
        self.device
    }

    fn kind(&self, node: u64) -> NodeKind {
        if node == ROOT {
            return NodeKind::Directory;
        }
        match self.volume.lock().entry_of(&self.layout, node) {
            Ok(Some((_, _, entry))) if entry.is_directory() => NodeKind::Directory,
            _ => NodeKind::Regular,
        }
    }

    fn attributes(&self, node: u64) -> Attributes {
        let directory = Attributes {
            kind: NodeKind::Directory,
            permissions: 0o755,
            file_id: ROOT_FILE_ID,
            links: 1,
            owner: 0,
            group: 0,
            size: 0,
            modified: 0,
        };
        if node == ROOT {
            return directory;
        }
        let Ok(Some((_, _, entry))) = self.volume.lock().entry_of(&self.layout, node) else {
            return Attributes {
                kind: NodeKind::Regular,
                file_id: node.wrapping_add(FILE_ID_OFFSET),
                ..directory
            };
        };
        let read_only = entry.attributes() & attribute::READ_ONLY != 0;
        let (kind, permissions, size) = match entry.is_directory() {
            true => (NodeKind::Directory, 0o755, 0),
            false if read_only => (NodeKind::Regular, 0o555, entry.size()),
            false => (NodeKind::Regular, 0o755, entry.size()),
        };
        Attributes {
            kind,
            permissions,
            file_id: node.wrapping_add(FILE_ID_OFFSET),
            size,
            modified: entry.modified(),
            ..directory
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        let mut volume = self.volume.lock();
        let key = self.key_of_directory(&mut volume, directory)?;
        let listed = directory_from_key(&self.layout, key);
        let mut from = 0;
        while let Some((index, entry)) = volume.next_short_entry(&self.layout, listed, from).ok()? {
            let (short, length) = entry.shown_name();
            let found = short[..length].eq_ignore_ascii_case(name)
                || volume
                    .name(&self.layout, listed, index, &entry)
                    .is_ok_and(|long| long.as_bytes().eq_ignore_ascii_case(name));
            if found {
                return Some(node_of(key, index));
            }
            from = index + 1;
        }
        None
    }

    fn parent(&self, node: u64) -> u64 {
        if node == ROOT {
            return ROOT;
        }
        let key = (node >> 32) as u32;
        if key == 0 || key == NO_DIRECTORY {
            return ROOT;
        }
        let mut volume = self.volume.lock();
        let layout = &self.layout;
        let dot_dot = volume.raw_entry(layout, Directory::Clusters(key), 1);
        let grandparent = match dot_dot {
            Ok(Some(raw)) => key_of(layout, &ShortEntry(raw)),
            _ => return ROOT,
        };
        let grandparent = match grandparent {
            NO_DIRECTORY => 0,
            key => key,
        };
        let listed = directory_from_key(layout, grandparent);
        let mut from = 0;
        while let Ok(Some((index, entry))) = volume.next_short_entry(layout, listed, from) {
            if entry.is_directory() && entry.first_cluster(layout.fat_type) == key {
                return node_of(grandparent, index);
            }
            from = index + 1;
        }
        ROOT
    }

    fn name(&self, node: u64) -> Name {
        let mut volume = self.volume.lock();
        match volume.entry_of(&self.layout, node) {
            Ok(Some((directory, index, entry))) => volume
                .name(&self.layout, directory, index, &entry)
                .unwrap_or(Name::new(b"")),
            _ => Name::new(b""),
        }
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn copies_contents(&self) -> bool {
        true
    }

    fn read_into(&self, node: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, StorageError> {
        let layout = &self.layout;
        let mut volume = self.volume.lock();
        let Some((_, _, entry)) = volume.entry_of(layout, node)? else {
            return Ok(0);
        };
        let size = entry.size();
        if entry.is_directory() || offset >= size || buffer.is_empty() {
            return Ok(0);
        }
        let wanted = (size - offset).min(buffer.len() as u64);
        let first = entry.first_cluster(layout.fat_type);
        let index = offset / layout.cluster_bytes;
        let within = offset % layout.cluster_bytes;
        let cluster = volume
            .cluster_in_chain(layout, false, first, index)?
            .ok_or(StorageError)?;
        let mut run = layout.cluster_bytes - within;
        let mut last = cluster;
        while run < wanted && run < MAX_RUN_BYTES {
            match volume.next_cluster(layout, last)? {
                Some(next) if next == last + 1 => {
                    last = next;
                    run += layout.cluster_bytes;
                }
                _ => break,
            }
        }
        let length = run.min(wanted) as usize;
        volume.read_data(
            layout.cluster_start(cluster) + within,
            &mut buffer[..length],
        )
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let layout = &self.layout;
        let mut volume = self.volume.lock();
        let key = self.key_of_directory(&mut volume, directory)?;
        let listed = directory_from_key(layout, key);
        let (mut at_position, mut found) = match volume.listing {
            Some(cursor) if cursor.directory == key && cursor.position <= position => {
                let raw = volume.raw_entry(layout, listed, cursor.index).ok()??;
                match classify(raw) {
                    Slot::Short(entry) => (cursor.position, (cursor.index, entry)),
                    _ => (0, volume.next_short_entry(layout, listed, 0).ok()??),
                }
            }
            _ => (0, volume.next_short_entry(layout, listed, 0).ok()??),
        };
        while at_position < position {
            found = volume
                .next_short_entry(layout, listed, found.0 + 1)
                .ok()??;
            at_position += 1;
        }
        let (index, entry) = found;
        volume.listing = Some(ListingCursor {
            directory: key,
            position,
            index,
        });
        let name = volume.name(layout, listed, index, &entry).ok()?;
        let kind = match entry.is_directory() {
            true => NodeKind::Directory,
            false => NodeKind::Regular,
        };
        Some(DirectoryEntry {
            name,
            node: node_of(key, index),
            kind,
        })
    }
}
