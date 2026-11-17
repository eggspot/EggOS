//! What is on a partitioned disk (docs/ux/installer-dual-boot-v0.1.md section 1, D634): its
//! table, its partitions with their types and file systems, the systems they hold, the free
//! runs between them, and where an EggOS installed alongside them would go. Every probe only
//! reads.

use crate::storage::{self, Fault, MIN_INSTALL_BYTES, SECTOR_BYTES, StorageError, WholeDisks};
use alloc::string::String;
use alloc::vec::Vec;
use eggos_gpt::{Guid, guid};

const MIB_SECTORS: u64 = (1 << 20) / SECTOR_BYTES;
const GIB: u64 = 1 << 30;
const PREFERRED_RUN_BYTES: u64 = 16 * GIB;
const MOST_RESERVE_BYTES: u64 = 8 * GIB;
const PROBE_BYTES: usize = 4096;
const BTRFS_AT: u64 = 0x1_0000;
const MBR_SIGNATURE_AT: usize = 510;
const MBR_TYPES_AT: usize = 446 + 4;
const MBR_ENTRY_BYTES: usize = 16;
const PROTECTIVE_MBR_TYPE: u8 = 0xee;

const MICROSOFT_RESERVED: Guid = guid(
    0xe3c9_e316,
    0x0b5c,
    0x4db8,
    [0x81, 0x7d, 0xf9, 0x2d, 0xf0, 0x02, 0x15, 0xae],
);
const MICROSOFT_DATA: Guid = guid(
    0xebd0_a0a2,
    0xb9e5,
    0x4433,
    [0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7],
);
const WINDOWS_RECOVERY: Guid = guid(
    0xde94_bba4,
    0x06d1,
    0x4d40,
    [0xa1, 0x6a, 0xbf, 0xd5, 0x01, 0x79, 0xd6, 0xac],
);
const APPLE_APFS: Guid = guid(
    0x7c34_57ef,
    0x0000,
    0x11aa,
    [0xaa, 0x11, 0x00, 0x30, 0x65, 0x43, 0xec, 0xac],
);
const APPLE_HFS: Guid = guid(
    0x4846_5300,
    0x0000,
    0x11aa,
    [0xaa, 0x11, 0x00, 0x30, 0x65, 0x43, 0xec, 0xac],
);
const LINUX_DATA: Guid = guid(
    0x0fc6_3daf,
    0x8483,
    0x4772,
    [0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47, 0x7d, 0xe4],
);
const LINUX_SWAP: Guid = guid(
    0x0657_fd6d,
    0xa4ab,
    0x43c4,
    [0x84, 0xe5, 0x09, 0x33, 0xc8, 0x4b, 0x4f, 0x4f],
);
const LINUX_LVM: Guid = guid(
    0xe6d6_d379,
    0xf507,
    0x44c2,
    [0xa2, 0x3c, 0x23, 0x8f, 0x2a, 0x3d, 0xf9, 0x28],
);

/// What kind of partition table a disk has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Gpt,
    Mbr,
    None,
    Damaged,
}

impl TableKind {
    /// Its name in `disk partitions`' record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gpt => "gpt",
            Self::Mbr => "mbr",
            Self::None => "none",
            Self::Damaged => "damaged",
        }
    }
}

/// A partition's type, from its GPT type GUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    EfiSystem,
    MicrosoftReserved,
    MicrosoftData,
    WindowsRecovery,
    AppleApfs,
    AppleHfs,
    LinuxData,
    LinuxSwap,
    LinuxLvm,
    EggosData,
    Unknown,
}

impl PartitionType {
    /// The type a GPT entry's type GUID names.
    pub fn of(kind: &Guid) -> Self {
        match *kind {
            eggos_gpt::EFI_SYSTEM_PARTITION => Self::EfiSystem,
            MICROSOFT_RESERVED => Self::MicrosoftReserved,
            MICROSOFT_DATA => Self::MicrosoftData,
            WINDOWS_RECOVERY => Self::WindowsRecovery,
            APPLE_APFS => Self::AppleApfs,
            APPLE_HFS => Self::AppleHfs,
            LINUX_DATA => Self::LinuxData,
            LINUX_SWAP => Self::LinuxSwap,
            LINUX_LVM => Self::LinuxLvm,
            eggos_gpt::EGGOS_DATA_PARTITION => Self::EggosData,
            _ => Self::Unknown,
        }
    }

    /// Its name in the record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::EfiSystem => "efi-system",
            Self::MicrosoftReserved => "microsoft-reserved",
            Self::MicrosoftData => "microsoft-data",
            Self::WindowsRecovery => "windows-recovery",
            Self::AppleApfs => "apple-apfs",
            Self::AppleHfs => "apple-hfs",
            Self::LinuxData => "linux-data",
            Self::LinuxSwap => "linux-swap",
            Self::LinuxLvm => "linux-lvm",
            Self::EggosData => "eggos-data",
            Self::Unknown => "unknown",
        }
    }
}

/// The file system a partition's first bytes show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemKind {
    Fat,
    Ntfs,
    Bitlocker,
    Apfs,
    HfsPlus,
    Ext4,
    Btrfs,
    Xfs,
    Luks,
    Swap,
    Eggfs,
    Unknown,
}

impl FileSystemKind {
    /// Its name in the record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fat => "fat",
            Self::Ntfs => "ntfs",
            Self::Bitlocker => "bitlocker",
            Self::Apfs => "apfs",
            Self::HfsPlus => "hfs+",
            Self::Ext4 => "ext4",
            Self::Btrfs => "btrfs",
            Self::Xfs => "xfs",
            Self::Luks => "luks",
            Self::Swap => "swap",
            Self::Eggfs => "eggfs",
            Self::Unknown => "unknown",
        }
    }

    /// The file system `start` (a partition's first [`PROBE_BYTES`]) and `btrfs` (its bytes at
    /// 64 KiB) show.
    pub fn probe(start: &[u8], btrfs: &[u8]) -> Self {
        let at = |offset: usize, magic: &[u8]| start.get(offset..offset + magic.len()) == Some(magic);
        match () {
            () if at(3, b"-FVE-FS-") => Self::Bitlocker,
            () if at(3, b"NTFS    ") => Self::Ntfs,
            () if at(0, b"EGGFS\0\0\0") => Self::Eggfs,
            () if at(32, b"NXSB") => Self::Apfs,
            () if at(1024, b"H+") || at(1024, b"HX") => Self::HfsPlus,
            () if at(1024 + 56, &[0x53, 0xef]) => Self::Ext4,
            () if at(0, b"XFSB") => Self::Xfs,
            () if at(0, b"LUKS\xba\xbe") => Self::Luks,
            () if at(PROBE_BYTES - 10, b"SWAPSPACE2") => Self::Swap,
            () if btrfs.get(64..72) == Some(b"_BHRfS_M") => Self::Btrfs,
            () if at(54, b"FAT") || at(82, b"FAT32") => Self::Fat,
            () => Self::Unknown,
        }
    }
}

/// A system EggOS recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKind {
    Windows,
    Macos,
    Linux,
    Eggos,
}

impl SystemKind {
    /// Its name in the record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Eggos => "eggos",
        }
    }

    /// Its name as people read it.
    pub const fn shown(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Macos => "macOS",
            Self::Linux => "Linux",
            Self::Eggos => "EggOS",
        }
    }
}

/// Why a partition cannot be shrunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShrinkBlocked {
    Encrypted,
    Hibernated,
    NeedsCheck,
    Unsupported,
}

impl ShrinkBlocked {
    /// Its name in the record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Encrypted => "encrypted",
            Self::Hibernated => "hibernated",
            Self::NeedsCheck => "needs-check",
            Self::Unsupported => "unsupported",
        }
    }
}

/// One partition of the table, numbered from 1 in table order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionRecord {
    pub number: usize,
    pub first_sector: u64,
    pub sectors: u64,
    pub kind: PartitionType,
    pub file_system: FileSystemKind,
    pub system: Option<SystemKind>,
    pub shrinkable_bytes: Option<u64>,
    pub shrink_blocked: Option<ShrinkBlocked>,
}

impl PartitionRecord {
    /// Its size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.sectors * SECTOR_BYTES
    }

    fn end(&self) -> u64 {
        self.first_sector + self.sectors
    }
}

/// A run of unallocated sectors of 1 MiB or more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeRun {
    pub first_sector: u64,
    pub sectors: u64,
}

impl FreeRun {
    /// Its size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.sectors * SECTOR_BYTES
    }
}

/// A system found, and the partition that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundSystem {
    pub system: SystemKind,
    pub partition: usize,
}

/// What `disk partitions` reports about a disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskLayout {
    pub table: TableKind,
    pub partitions: Vec<PartitionRecord>,
    pub unallocated: Vec<FreeRun>,
    pub systems: Vec<FoundSystem>,
}

/// The reserve a shrunk system keeps of its free space: `min(8 GiB, free / 2)`.
pub fn reserve_of(free_bytes: u64) -> u64 {
    MOST_RESERVE_BYTES.min(free_bytes / 2)
}

/// The free runs of 1 MiB or more between `partitions` (any order) within the usable sectors
/// `first_usable ..= last_usable`, aligned to 1 MiB.
pub fn free_runs(partitions: &[PartitionRecord], first_usable: u64, last_usable: u64) -> Vec<FreeRun> {
    let mut taken: Vec<(u64, u64)> = partitions
        .iter()
        .map(|partition| (partition.first_sector, partition.end()))
        .collect();
    taken.sort_unstable();
    let mut runs = Vec::new();
    let mut at = first_usable;
    let end = last_usable.saturating_add(1);
    for (start, stop) in taken.into_iter().chain(core::iter::once((end, end))) {
        let first = at.next_multiple_of(MIB_SECTORS);
        let last = start.min(end) / MIB_SECTORS * MIB_SECTORS;
        if last > first && last - first >= MIB_SECTORS {
            runs.push(FreeRun {
                first_sector: first,
                sectors: last - first,
            });
        }
        at = at.max(stop);
    }
    runs
}

/// The systems `partitions` hold, by the recognition table: Windows on the largest Microsoft
/// data partition that is NTFS or BitLocker when the disk has a Microsoft reserved partition
/// or such a data partition; macOS on an APFS or HFS+ partition; Linux on the largest Linux
/// data partition; EggOS on its data partition.
pub fn recognise(partitions: &mut [PartitionRecord]) -> Vec<FoundSystem> {
    let has_reserved = partitions
        .iter()
        .any(|partition| partition.kind == PartitionType::MicrosoftReserved);
    let largest = |partitions: &[PartitionRecord], matches: &dyn Fn(&PartitionRecord) -> bool| {
        partitions
            .iter()
            .filter(|partition| matches(partition))
            .max_by_key(|partition| partition.sectors)
            .map(|partition| partition.number)
    };
    let windows = largest(partitions, &|partition| {
        partition.kind == PartitionType::MicrosoftData
            && matches!(
                partition.file_system,
                FileSystemKind::Ntfs | FileSystemKind::Bitlocker
            )
    })
    .filter(|_| {
        has_reserved
            || partitions
                .iter()
                .any(|partition| partition.kind == PartitionType::MicrosoftData)
    });
    let macos = partitions
        .iter()
        .find(|partition| matches!(partition.kind, PartitionType::AppleApfs | PartitionType::AppleHfs))
        .map(|partition| partition.number);
    let linux = largest(partitions, &|partition| partition.kind == PartitionType::LinuxData);
    let eggos = partitions
        .iter()
        .find(|partition| partition.kind == PartitionType::EggosData)
        .map(|partition| partition.number);
    let found: Vec<FoundSystem> = [
        (SystemKind::Windows, windows),
        (SystemKind::Linux, linux),
        (SystemKind::Macos, macos),
        (SystemKind::Eggos, eggos),
    ]
    .into_iter()
    .filter_map(|(system, partition)| Some(FoundSystem { system, partition: partition? }))
    .collect();
    for system in &found {
        if let Some(partition) = partitions
            .iter_mut()
            .find(|partition| partition.number == system.partition)
        {
            partition.system = Some(system.system);
        }
    }
    found
}

/// The systems' names as people say them: `Windows`, `Windows and Linux`, `Windows, Linux and
/// macOS`.
pub fn joined_names(systems: &[FoundSystem]) -> String {
    let names: Vec<&str> = systems.iter().map(|found| found.system.shown()).collect();
    match names.as_slice() {
        [] => String::new(),
        [only] => String::from(*only),
        [first @ .., last] => {
            let mut text = first.join(", ");
            text.push_str(" and ");
            text.push_str(last);
            text
        }
    }
}

/// The Storage pane's contents line for a partitioned disk: `Windows · 4 partitions`, or
/// `3 partitions` with no system EggOS knows.
pub fn contents_line(layout: &DiskLayout) -> String {
    let count = layout.partitions.len();
    let partitions = match count {
        1 => String::from("1 partition"),
        count => alloc::format!("{count} partitions"),
    };
    match layout.systems.is_empty() {
        true => partitions,
        false => alloc::format!("{} \u{b7} {partitions}", joined_names(&layout.systems)),
    }
}

/// Where an EggOS installed alongside would go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// In this free run, as it is.
    Unallocated(FreeRun),
    /// In the free tail of this partition, shrunk by up to `shrink_bytes`, joined with the
    /// free run that follows it, if any.
    ShrunkTail {
        partition: usize,
        shrink_bytes: u64,
        following: Option<FreeRun>,
    },
}

impl Placement {
    /// The room it gives, in whole GiB.
    pub fn room_gib(&self) -> u64 {
        match *self {
            Self::Unallocated(run) => run.size_bytes() / GIB,
            Self::ShrunkTail {
                shrink_bytes,
                following,
                ..
            } => (shrink_bytes + following.map_or(0, |run| run.size_bytes())) / GIB,
        }
    }
}

/// Why installing alongside is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlongsideRefusal {
    TableIsMbr,
    TableDamaged,
    NoRoom,
}

impl AlongsideRefusal {
    /// Its code, for the command's and the tool's errors.
    pub const fn code(self) -> &'static str {
        match self {
            Self::TableIsMbr => "table_is_mbr",
            Self::TableDamaged => "table_damaged",
            Self::NoRoom => "no_room_alongside",
        }
    }
}

/// Where EggOS goes beside what `layout` holds: the largest free run of 16 GiB or more; else
/// the partition with the most shrinkable bytes, joined with the run right after it; else the
/// largest free run of 1 GiB or more.
pub fn place(layout: &DiskLayout) -> Result<Placement, AlongsideRefusal> {
    match layout.table {
        TableKind::Mbr => return Err(AlongsideRefusal::TableIsMbr),
        TableKind::Damaged => return Err(AlongsideRefusal::TableDamaged),
        TableKind::Gpt | TableKind::None => {}
    }
    let largest = layout
        .unallocated
        .iter()
        .copied()
        .max_by_key(|run| run.sectors);
    if let Some(run) = largest.filter(|run| run.size_bytes() >= PREFERRED_RUN_BYTES) {
        return Ok(Placement::Unallocated(run));
    }
    let shrinkable = layout
        .partitions
        .iter()
        .filter_map(|partition| Some((partition, partition.shrinkable_bytes?)))
        .filter(|(_, bytes)| *bytes > 0)
        .max_by_key(|(_, bytes)| *bytes);
    if let Some((partition, shrink_bytes)) = shrinkable {
        let end = partition.end().next_multiple_of(MIB_SECTORS);
        let following = layout
            .unallocated
            .iter()
            .copied()
            .find(|run| run.first_sector == end);
        let placement = Placement::ShrunkTail {
            partition: partition.number,
            shrink_bytes,
            following,
        };
        if placement.room_gib() * GIB >= MIN_INSTALL_BYTES {
            return Ok(placement);
        }
    }
    match largest.filter(|run| run.size_bytes() >= MIN_INSTALL_BYTES) {
        Some(run) => Ok(Placement::Unallocated(run)),
        None => Err(AlongsideRefusal::NoRoom),
    }
}

/// The table kind sector 0 (`mbr`) and sector 1 (`gpt_header`) show: a GPT when the header
/// reads, damaged when the protective MBR is there without one, an MBR when sector 0 carries
/// partitions of its own, none otherwise.
pub fn table_kind(mbr: &[u8], gpt_header: Option<&eggos_gpt::Header>) -> TableKind {
    let signed = mbr.get(MBR_SIGNATURE_AT..MBR_SIGNATURE_AT + 2) == Some(&[0x55, 0xaa][..]);
    let types: Vec<u8> = (0..4)
        .filter_map(|index| mbr.get(MBR_TYPES_AT + index * MBR_ENTRY_BYTES).copied())
        .collect();
    let protective = signed && types.contains(&PROTECTIVE_MBR_TYPE);
    match (gpt_header.is_some(), protective, signed && types.iter().any(|&kind| kind != 0)) {
        (true, _, _) => TableKind::Gpt,
        (false, true, _) => TableKind::Damaged,
        (false, false, true) => TableKind::Mbr,
        _ => TableKind::None,
    }
}

/// The layout of attached disk `device` (`/dev/vdb` or `vdb`): what `disk partitions`
/// reports, with the disk's device file.
pub fn disk_layout(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
) -> Result<(&'static str, DiskLayout), StorageError> {
    let record = storage::find(disks, device)?;
    let layout = read_layout(disks, record.device, record.size_bytes / SECTOR_BYTES)
        .map_err(StorageError::of)?;
    Ok((record.device, layout))
}

/// Reads `device`'s table and partitions through `disks` and probes each partition's file
/// system: what `disk partitions` reports.
pub fn read_layout(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
    disk_sectors: u64,
) -> Result<DiskLayout, Fault> {
    let handle = disks.open(device, false)?;
    let layout = read_open_layout(disks, handle, disk_sectors);
    disks.close(handle);
    layout
}

fn read_open_layout(
    disks: &mut (impl WholeDisks + ?Sized),
    handle: usize,
    disk_sectors: u64,
) -> Result<DiskLayout, Fault> {
    let mut mbr = [0u8; SECTOR_BYTES as usize];
    disks.read_at(handle, 0, &mut mbr)?;
    let mut header_block = [0u8; SECTOR_BYTES as usize];
    disks.read_at(handle, SECTOR_BYTES, &mut header_block)?;
    let header = eggos_gpt::read_header(&header_block);
    let table = table_kind(&mbr, header.as_ref());
    let Some(header) = header else {
        return Ok(DiskLayout {
            table,
            partitions: Vec::new(),
            unallocated: Vec::new(),
            systems: Vec::new(),
        });
    };
    let array_bytes = header.entry_count as usize * header.entry_bytes as usize;
    let mut entries = Vec::new();
    entries.resize(array_bytes, 0);
    disks.read_at(handle, header.entries_at * SECTOR_BYTES, &mut entries)?;
    let Some(found) = eggos_gpt::partitions(&header, &entries) else {
        return Ok(DiskLayout {
            table: TableKind::Damaged,
            partitions: Vec::new(),
            unallocated: Vec::new(),
            systems: Vec::new(),
        });
    };
    let found: Vec<eggos_gpt::Partition> = found.collect();
    let mut partitions = Vec::new();
    for (index, partition) in found.iter().enumerate() {
        let mut start = [0u8; PROBE_BYTES];
        let mut btrfs = [0u8; 72];
        let offset = partition.first * SECTOR_BYTES;
        disks.read_at(handle, offset, &mut start)?;
        if partition.blocks() * SECTOR_BYTES > BTRFS_AT + 72 {
            disks.read_at(handle, offset + BTRFS_AT, &mut btrfs)?;
        }
        let file_system = FileSystemKind::probe(&start, &btrfs);
        let shrink_blocked = match file_system {
            FileSystemKind::Bitlocker | FileSystemKind::Luks => Some(ShrinkBlocked::Encrypted),
            _ => Some(ShrinkBlocked::Unsupported),
        };
        partitions.push(PartitionRecord {
            number: index + 1,
            first_sector: partition.first,
            sectors: partition.blocks(),
            kind: PartitionType::of(&partition.kind),
            file_system,
            system: None,
            shrinkable_bytes: None,
            shrink_blocked,
        });
    }
    let last_usable = header.last_usable.min(disk_sectors.saturating_sub(1));
    let unallocated = free_runs(&partitions, header.first_usable, last_usable);
    let systems = recognise(&mut partitions);
    Ok(DiskLayout {
        table,
        partitions,
        unallocated,
        systems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partition(number: usize, first: u64, sectors: u64, kind: PartitionType) -> PartitionRecord {
        PartitionRecord {
            number,
            first_sector: first,
            sectors,
            kind,
            file_system: match kind {
                PartitionType::MicrosoftData => FileSystemKind::Ntfs,
                PartitionType::EfiSystem => FileSystemKind::Fat,
                PartitionType::LinuxData => FileSystemKind::Ext4,
                _ => FileSystemKind::Unknown,
            },
            system: None,
            shrinkable_bytes: None,
            shrink_blocked: None,
        }
    }

    fn windows_disk() -> Vec<PartitionRecord> {
        let gib = GIB / SECTOR_BYTES;
        vec![
            partition(1, 2048, 200 * MIB_SECTORS, PartitionType::EfiSystem),
            partition(2, 2048 + 200 * MIB_SECTORS, 16 * MIB_SECTORS, PartitionType::MicrosoftReserved),
            partition(3, 2048 + 216 * MIB_SECTORS, 100 * gib, PartitionType::MicrosoftData),
            partition(4, 2048 + 216 * MIB_SECTORS + 100 * gib, gib, PartitionType::WindowsRecovery),
        ]
    }

    #[test]
    fn windows_is_found_on_its_largest_data_partition_and_named_with_the_count() {
        let mut partitions = windows_disk();
        let mut linux = partition(5, 300 * (GIB / SECTOR_BYTES), 20 * (GIB / SECTOR_BYTES), PartitionType::LinuxData);
        linux.file_system = FileSystemKind::Ext4;
        partitions.push(linux);
        let systems = recognise(&mut partitions);
        assert_eq!(systems[0], FoundSystem { system: SystemKind::Windows, partition: 3 });
        assert_eq!(partitions[2].system, Some(SystemKind::Windows));
        let layout = DiskLayout {
            table: TableKind::Gpt,
            partitions,
            unallocated: Vec::new(),
            systems,
        };
        assert_eq!(contents_line(&layout), "Windows and Linux \u{b7} 5 partitions");
    }

    #[test]
    fn free_runs_are_aligned_and_at_least_a_mebibyte() {
        let partitions = windows_disk();
        let end_of_last = partitions[3].end();
        let disk_end = end_of_last + 40 * (GIB / SECTOR_BYTES);
        let runs = free_runs(&partitions, 34, disk_end - 34);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first_sector, end_of_last.next_multiple_of(MIB_SECTORS));
        assert!(runs[0].size_bytes() >= 39 * GIB);
    }

    #[test]
    fn placement_prefers_a_large_free_run_then_a_shrinkable_tail_then_any_gibibyte() {
        let mut partitions = windows_disk();
        let mut layout = DiskLayout {
            table: TableKind::Gpt,
            partitions: partitions.clone(),
            unallocated: vec![FreeRun {
                first_sector: 1 << 30,
                sectors: 20 * (GIB / SECTOR_BYTES),
            }],
            systems: Vec::new(),
        };
        assert!(matches!(place(&layout), Ok(Placement::Unallocated(_))));
        layout.unallocated[0].sectors = 2 * (GIB / SECTOR_BYTES);
        partitions[2].shrinkable_bytes = Some(30 * GIB);
        layout.partitions = partitions;
        assert!(matches!(
            place(&layout),
            Ok(Placement::ShrunkTail { partition: 3, .. })
        ));
        layout.partitions[2].shrinkable_bytes = None;
        assert_eq!(place(&layout).map(|placement| placement.room_gib()), Ok(2));
        layout.unallocated.clear();
        assert_eq!(place(&layout), Err(AlongsideRefusal::NoRoom));
        layout.table = TableKind::Mbr;
        assert_eq!(place(&layout), Err(AlongsideRefusal::TableIsMbr));
    }

    #[test]
    fn file_systems_are_known_by_their_first_bytes() {
        let mut start = [0u8; PROBE_BYTES];
        start[3..11].copy_from_slice(b"NTFS    ");
        assert_eq!(FileSystemKind::probe(&start, &[]), FileSystemKind::Ntfs);
        let mut start = [0u8; PROBE_BYTES];
        start[1024 + 56..1024 + 58].copy_from_slice(&[0x53, 0xef]);
        assert_eq!(FileSystemKind::probe(&start, &[]), FileSystemKind::Ext4);
        let mut btrfs = [0u8; 72];
        btrfs[64..72].copy_from_slice(b"_BHRfS_M");
        assert_eq!(FileSystemKind::probe(&[0u8; PROBE_BYTES], &btrfs), FileSystemKind::Btrfs);
        assert_eq!(reserve_of(40 * GIB), 8 * GIB);
        assert_eq!(reserve_of(6 * GIB), 3 * GIB);
    }

    #[test]
    fn the_table_kind_comes_from_the_header_and_the_mbr() {
        let mut mbr = [0u8; 512];
        assert_eq!(table_kind(&mbr, None), TableKind::None);
        mbr[510] = 0x55;
        mbr[511] = 0xaa;
        mbr[MBR_TYPES_AT] = 0x07;
        assert_eq!(table_kind(&mbr, None), TableKind::Mbr);
        mbr[MBR_TYPES_AT] = PROTECTIVE_MBR_TYPE;
        assert_eq!(table_kind(&mbr, None), TableKind::Damaged);
    }
}
