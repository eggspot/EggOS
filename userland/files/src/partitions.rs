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
    /// Its name in the table, empty for none.
    pub label: String,
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
    /// Its name as people read it: the distribution's for Linux when the EFI system
    /// partition names one.
    pub name: &'static str,
    pub partition: usize,
}

/// What the EFI system partition's folders say (D640): whether Windows's boot manager is
/// there, and the Linux distribution whose folder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EspFindings {
    pub windows_boot_manager: bool,
    pub distribution: Option<&'static str>,
}

/// The distributions known by their folder under `\EFI`, with the names shown.
pub const DISTRIBUTIONS: [(&str, &str); 10] = [
    ("ubuntu", "Ubuntu"),
    ("fedora", "Fedora"),
    ("debian", "Debian"),
    ("opensuse", "openSUSE"),
    ("arch", "Arch Linux"),
    ("centos", "CentOS"),
    ("rocky", "Rocky Linux"),
    ("almalinux", "AlmaLinux"),
    ("linuxmint", "Linux Mint"),
    ("pop", "Pop!_OS"),
];

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
pub fn recognise(partitions: &mut [PartitionRecord], esp: EspFindings) -> Vec<FoundSystem> {
    let has_reserved = esp.windows_boot_manager
        || partitions
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
    let linux = largest(partitions, &|partition| partition.kind == PartitionType::LinuxData)
        .or_else(|| {
            largest(partitions, &|partition| {
                matches!(partition.kind, PartitionType::LinuxLvm | PartitionType::LinuxSwap)
            })
        });
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
    .filter_map(|(system, partition)| {
        let name = match (system, esp.distribution) {
            (SystemKind::Linux, Some(distribution)) => distribution,
            _ => system.shown(),
        };
        Some(FoundSystem {
            system,
            name,
            partition: partition?,
        })
    })
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
    let names: Vec<&str> = systems.iter().map(|found| found.name).collect();
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
    TableFull,
    PartitionEncrypted,
    SystemHibernated,
    NeedsCheck,
    CannotShrink,
    NoRoom,
}

impl AlongsideRefusal {
    /// Its code, for the command's and the tool's errors.
    pub const fn code(self) -> &'static str {
        match self {
            Self::TableIsMbr => "table_is_mbr",
            Self::TableDamaged => "table_damaged",
            Self::TableFull => "table_full",
            Self::PartitionEncrypted => "partition_encrypted",
            Self::SystemHibernated => "system_hibernated",
            Self::NeedsCheck => "needs_check",
            Self::CannotShrink => "cannot_shrink",
            Self::NoRoom => "no_room_alongside",
        }
    }
}

/// The most partitions a GPT EggOS writes holds, and so the most it adds one to.
pub const TABLE_ENTRIES: usize = 128;

fn no_room_reason(layout: &DiskLayout) -> AlongsideRefusal {
    let largest = layout
        .systems
        .iter()
        .filter_map(|found| {
            layout
                .partitions
                .iter()
                .find(|partition| partition.number == found.partition)
        })
        .max_by_key(|partition| partition.sectors);
    let Some(partition) = largest else {
        return AlongsideRefusal::NoRoom;
    };
    match (partition.shrink_blocked, partition.file_system) {
        (Some(ShrinkBlocked::Encrypted), _) => AlongsideRefusal::PartitionEncrypted,
        (Some(ShrinkBlocked::Hibernated), _) => AlongsideRefusal::SystemHibernated,
        (Some(ShrinkBlocked::NeedsCheck), _) => AlongsideRefusal::NeedsCheck,
        (
            _,
            FileSystemKind::Apfs
            | FileSystemKind::HfsPlus
            | FileSystemKind::Btrfs
            | FileSystemKind::Xfs
            | FileSystemKind::Luks,
        ) => AlongsideRefusal::CannotShrink,
        (_, _) if partition.kind == PartitionType::LinuxLvm => AlongsideRefusal::CannotShrink,
        _ => AlongsideRefusal::NoRoom,
    }
}

/// The two lines that say why installing alongside what `layout` holds is refused and the
/// way out (section 8), for the pane's two lines, the command's message and the tool's error.
pub fn refusal_lines(refusal: AlongsideRefusal, layout: &DiskLayout) -> (String, String) {
    let largest = layout
        .systems
        .iter()
        .filter_map(|found| {
            let partition = layout
                .partitions
                .iter()
                .find(|partition| partition.number == found.partition)?;
            Some((found.name, partition))
        })
        .max_by_key(|(_, partition)| partition.sectors);
    let name = largest.map_or("the other system", |(name, _)| name);
    let file_system = largest.map_or("its", |(_, partition)| match partition.file_system {
        FileSystemKind::Apfs => "APFS",
        FileSystemKind::HfsPlus => "HFS+",
        FileSystemKind::Btrfs => "Btrfs",
        FileSystemKind::Xfs => "XFS",
        FileSystemKind::Luks => "LUKS",
        _ => "LVM",
    });
    let (first, second) = match refusal {
        AlongsideRefusal::TableIsMbr => (
            String::from("MBR partition table: EggOS installs alongside on GPT disks only."),
            String::from("Convert it to GPT in the other system, or use the whole disk."),
        ),
        AlongsideRefusal::TableDamaged => (
            String::from("The partition table is damaged: EggOS will not change it."),
            String::from("Repair it in the other system, or use the whole disk."),
        ),
        AlongsideRefusal::TableFull => (
            alloc::format!("The partition table is full: {TABLE_ENTRIES} partitions."),
            String::from("Remove one in the other system, or use the whole disk."),
        ),
        AlongsideRefusal::PartitionEncrypted => (
            alloc::format!("{name} is encrypted with BitLocker: EggOS cannot shrink it."),
            alloc::format!("In {name}, shrink the volume by 16 GB or more, then come back."),
        ),
        AlongsideRefusal::SystemHibernated => (
            alloc::format!("{name} is hibernated or uses Fast Startup: its disk is in use."),
            alloc::format!("Shut {name} down fully (shutdown /s /t 0), then try again."),
        ),
        AlongsideRefusal::NeedsCheck => (
            alloc::format!("{name}' file system needs a check: EggOS will not shrink it."),
            String::from("Run chkdsk in Windows (fsck in Linux), then try again."),
        ),
        AlongsideRefusal::CannotShrink => (
            alloc::format!("EggOS cannot shrink a {name} ({file_system}) volume."),
            alloc::format!("Shrink it in {name}, then install in the space."),
        ),
        AlongsideRefusal::NoRoom => (
            alloc::format!("No room beside {name}: EggOS needs 1 GB."),
            alloc::format!("Free space in {name}, or use the whole disk."),
        ),
    };
    (first, second)
}

/// The size, in whole GiB, the alongside slider opens at for `placement` (section 2.1): all
/// of an unallocated run; else half the room, or 16 when half is less and the room allows 16,
/// or all the room when it is under 16.
pub fn opening_size_gib(placement: &Placement) -> u64 {
    let room = placement.room_gib();
    match placement {
        Placement::Unallocated(_) => room,
        Placement::ShrunkTail { .. } if room < 16 => room,
        Placement::ShrunkTail { .. } => (room / 2).max(16),
    }
}

/// Where EggOS goes beside what `layout` holds: the largest free run of 16 GiB or more; else
/// the partition with the most shrinkable bytes, joined with the run right after it; else the
/// largest free run of 1 GiB or more.
pub fn place(layout: &DiskLayout) -> Result<Placement, AlongsideRefusal> {
    match layout.table {
        TableKind::Mbr => return Err(AlongsideRefusal::TableIsMbr),
        TableKind::Damaged => return Err(AlongsideRefusal::TableDamaged),
        TableKind::Gpt if layout.partitions.len() >= TABLE_ENTRIES => {
            return Err(AlongsideRefusal::TableFull);
        }
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
        None => Err(no_room_reason(layout)),
    }
}

fn whole_gb(bytes: u64) -> u64 {
    bytes / GIB
}

/// The bytes `disk install --size` asks for: whole GiB or TiB with a unit (`36G`, `1T`), or a
/// count of bytes that is a whole number of GiB, as `disk_install`'s `size_bytes` gives it.
pub fn size_bytes_of(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, unit) = match text.char_indices().find(|(_, character)| !character.is_ascii_digit()) {
        Some((at, _)) => (&text[..at], &text[at..]),
        None => (text, ""),
    };
    let number: u64 = digits.parse().ok()?;
    let bytes = match unit.to_ascii_uppercase().as_str() {
        "G" | "GB" | "GIB" => number.checked_mul(GIB)?,
        "T" | "TB" | "TIB" => number.checked_mul(GIB * 1024)?,
        "" if number % GIB == 0 => number,
        _ => return None,
    };
    (bytes > 0).then_some(bytes)
}

/// The sizes, in whole GiB, EggOS may take at `placement`: 1 to all the room.
pub fn size_range_gib(placement: &Placement) -> (u64, u64) {
    (MIN_INSTALL_BYTES.div_ceil(GIB), placement.room_gib())
}

fn system_named(layout: &DiskLayout, partition: usize) -> &'static str {
    layout
        .systems
        .iter()
        .find(|found| found.partition == partition)
        .map_or("the partition", |found| found.name)
}

fn efi_system_partition(layout: &DiskLayout) -> Option<usize> {
    layout
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionType::EfiSystem)
        .map(|partition| partition.number)
}

fn shrunk_sizes(layout: &DiskLayout, partition: usize, taken_bytes: u64, following: Option<FreeRun>) -> (u64, u64) {
    let size = layout
        .partitions
        .iter()
        .find(|found| found.number == partition)
        .map_or(0, PartitionRecord::size_bytes);
    let from_following = following.map_or(0, |run| run.size_bytes()).min(taken_bytes);
    (size, size.saturating_sub(taken_bytes - from_following))
}

/// The dry run's sentence for installing `size_gib` GiB alongside at `placement` (section 7):
/// what is shrunk, written and added.
pub fn alongside_dry_run_text(layout: &DiskLayout, placement: &Placement, size_gib: u64) -> String {
    let taken = size_gib * GIB;
    let made = match *placement {
        Placement::Unallocated(_) => alloc::format!("write an EggOS partition of {size_gib} GB in unallocated space"),
        Placement::ShrunkTail { partition, following, .. } => {
            let (before, after) = shrunk_sizes(layout, partition, taken, following);
            if after == before {
                alloc::format!("write an EggOS partition of {size_gib} GB in unallocated space")
            } else {
                alloc::format!(
                    "shrink {} (partition {partition}) from {} GB to {} GB, write an EggOS partition of {size_gib} GB",
                    system_named(layout, partition),
                    whole_gb(before),
                    whole_gb(after)
                )
            }
        }
    };
    let boot = match efi_system_partition(layout) {
        Some(number) => alloc::format!("add EggOS to the EFI system partition (partition {number})"),
        None => String::from("write an EFI system partition of 256 MB"),
    };
    alloc::format!("would {made}, {boot} and add a boot entry")
}

/// The question `disk install --alongside` asks on a terminal (section 7).
pub fn alongside_question(short: &str, device: &str, layout: &DiskLayout, placement: &Placement, size_gib: u64) -> String {
    let kept = joined_names(&layout.systems);
    let kept = if kept.is_empty() { String::from("what is on it") } else { kept };
    let what = match *placement {
        Placement::ShrunkTail { partition, following, .. } => {
            let (before, after) = shrunk_sizes(layout, partition, size_gib * GIB, following);
            if after == before {
                alloc::format!("uses {size_gib} GB of unallocated space")
            } else {
                alloc::format!("shrinks {} to {} GB", system_named(layout, partition), whole_gb(after))
            }
        }
        Placement::Unallocated(_) => alloc::format!("uses {size_gib} GB of unallocated space"),
    };
    alloc::format!("Type {short} to install EggOS beside {kept} on {device} ({what}, erases nothing): ")
}

/// What a whole-disk install erases, naming the systems (section 7): `Windows and 3 more
/// partitions`; `None` when no system is on the disk.
pub fn erased_systems_text(layout: &DiskLayout) -> Option<String> {
    if layout.systems.is_empty() {
        return None;
    }
    let names = joined_names(&layout.systems);
    let holding: Vec<usize> = layout.systems.iter().map(|found| found.partition).collect();
    let more = layout
        .partitions
        .iter()
        .filter(|partition| !holding.contains(&partition.number))
        .count();
    Some(match more {
        0 => names,
        1 => alloc::format!("{names} and 1 more partition"),
        _ => alloc::format!("{names} and {more} more partitions"),
    })
}

/// The hint a terminal gets before a whole-disk install of a disk that holds a system
/// (section 7).
pub fn keep_hint(layout: &DiskLayout, device: &str) -> Option<String> {
    if layout.systems.is_empty() {
        return None;
    }
    let names = joined_names(&layout.systems);
    let verb = if layout.systems.len() == 1 { "is" } else { "are" };
    Some(alloc::format!("{names} {verb} on this disk. To keep it: disk install --alongside {device}"))
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
    let Some(found) = eggos_gpt::named_partitions(&header, &entries) else {
        return Ok(DiskLayout {
            table: TableKind::Damaged,
            partitions: Vec::new(),
            unallocated: Vec::new(),
            systems: Vec::new(),
        });
    };
    let found: Vec<(eggos_gpt::Partition, [u16; eggos_gpt::NAME_UNITS])> = found.collect();
    let mut partitions = Vec::new();
    for (index, (partition, name)) in found.iter().enumerate() {
        let mut start = [0u8; PROBE_BYTES];
        let mut btrfs = [0u8; 72];
        let offset = partition.first * SECTOR_BYTES;
        disks.read_at(handle, offset, &mut start)?;
        if partition.blocks() * SECTOR_BYTES > BTRFS_AT + 72 {
            disks.read_at(handle, offset + BTRFS_AT, &mut btrfs)?;
        }
        let file_system = FileSystemKind::probe(&start, &btrfs);
        let partition_bytes = partition.blocks() * SECTOR_BYTES;
        let tail = match file_system {
            FileSystemKind::Ext4 => {
                let read = &mut |at: u64, buffer: &mut [u8]| {
                    disks.read_at(handle, offset + at, buffer).is_ok()
                };
                ext4_tail(&start, partition_bytes, read)
            }
            FileSystemKind::Ntfs => {
                let read = &mut |at: u64, buffer: &mut [u8]| {
                    disks.read_at(handle, offset + at, buffer).is_ok()
                };
                ntfs_tail(&start, partition_bytes, read)
            }
            _ => None,
        };
        let (shrinkable_bytes, shrink_blocked) = match (file_system, tail) {
            (FileSystemKind::Bitlocker | FileSystemKind::Luks, _) => {
                (None, Some(ShrinkBlocked::Encrypted))
            }
            (_, Some(Ok(bytes))) => (Some(bytes), None),
            (_, Some(Err(reason))) => (None, Some(reason)),
            (_, None) => (None, Some(ShrinkBlocked::Unsupported)),
        };
        partitions.push(PartitionRecord {
            number: index + 1,
            first_sector: partition.first,
            sectors: partition.blocks(),
            kind: PartitionType::of(&partition.kind),
            file_system,
            label: label_of(name),
            system: None,
            shrinkable_bytes,
            shrink_blocked,
        });
    }
    let last_usable = header.last_usable.min(disk_sectors.saturating_sub(1));
    let unallocated = free_runs(&partitions, header.first_usable, last_usable);
    let mut esp = EspFindings::default();
    for partition in partitions
        .iter()
        .filter(|partition| partition.kind == PartitionType::EfiSystem)
        .filter(|partition| partition.file_system == FileSystemKind::Fat)
    {
        let offset = partition.first_sector * SECTOR_BYTES;
        let read = &mut |at: u64, buffer: &mut [u8]| disks.read_at(handle, offset + at, buffer).is_ok();
        let found = esp_findings(read).unwrap_or_default();
        esp.windows_boot_manager |= found.windows_boot_manager;
        esp.distribution = esp.distribution.or(found.distribution);
    }
    let systems = recognise(&mut partitions, esp);
    Ok(DiskLayout {
        table,
        partitions,
        unallocated,
        systems,
    })
}

const EXT4_SUPERBLOCK_AT: u64 = 1024;
const EXT4_STATE_CLEAN: u16 = 1;
const EXT4_STATE_ERRORS: u16 = 2;
const EXT4_INCOMPAT_64BIT: u32 = 0x80;
const EXT4_GROUP_BLOCK_UNINIT: u16 = 2;
const EXT4_SMALL_DESCRIPTOR: usize = 32;
const EXT4_MOST_BLOCK_SHIFT: u32 = 6;

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// How much of an ext4 partition of `partition_bytes` can be taken from its end without moving
/// a file (D637): the space after its last used block, less the reserve its free space keeps;
/// `start` is its first 4 KiB and `read` reads its bytes at an offset. `needs-check` for a
/// volume not cleanly unmounted or with errors recorded; None where the volume cannot be read.
pub fn ext4_tail(
    start: &[u8],
    partition_bytes: u64,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
) -> Option<Result<u64, ShrinkBlocked>> {
    let superblock = start.get(EXT4_SUPERBLOCK_AT as usize..)?;
    let state = le16(superblock, 58)?;
    if state & EXT4_STATE_CLEAN == 0 || state & EXT4_STATE_ERRORS != 0 {
        return Some(Err(ShrinkBlocked::NeedsCheck));
    }
    let wide = le32(superblock, 96)? & EXT4_INCOMPAT_64BIT != 0;
    let high = |at: usize| -> Option<u64> {
        Some(match wide {
            true => u64::from(le32(superblock, at)?) << 32,
            false => 0,
        })
    };
    let blocks = u64::from(le32(superblock, 4)?) | high(336)?;
    let free_blocks = u64::from(le32(superblock, 12)?) | high(344)?;
    let first_data_block = u64::from(le32(superblock, 20)?);
    let shift = le32(superblock, 24)?;
    if shift > EXT4_MOST_BLOCK_SHIFT {
        return None;
    }
    let block_bytes = 1024u64 << shift;
    let per_group = u64::from(le32(superblock, 32)?);
    if per_group == 0 || blocks == 0 {
        return None;
    }
    let descriptor_bytes = match wide {
        true => usize::from(le16(superblock, 254)?).max(EXT4_SMALL_DESCRIPTOR),
        false => EXT4_SMALL_DESCRIPTOR,
    };
    let groups = (blocks - first_data_block).div_ceil(per_group);
    let table_at = (first_data_block + 1) * block_bytes;
    let mut bitmap = Vec::new();
    bitmap.resize(block_bytes as usize, 0);
    let mut last_used = None;
    for group in (0..groups).rev() {
        let mut descriptor = [0u8; 64];
        let descriptor = &mut descriptor[..descriptor_bytes.min(64)];
        if !read(table_at + group * descriptor_bytes as u64, descriptor) {
            return None;
        }
        let group_start = first_data_block + group * per_group;
        let in_group = per_group.min(blocks - group_start);
        let flags = le16(descriptor, 0x12)?;
        if flags & EXT4_GROUP_BLOCK_UNINIT != 0 {
            let free = u64::from(le16(descriptor, 0x0c)?);
            let used = in_group.saturating_sub(free);
            if used > 0 {
                last_used = Some(group_start + used - 1);
                break;
            }
            continue;
        }
        let bitmap_block = u64::from(le32(descriptor, 0)?)
            | match descriptor_bytes > EXT4_SMALL_DESCRIPTOR {
                true => u64::from(le32(descriptor, 0x20)?) << 32,
                false => 0,
            };
        if !read(bitmap_block * block_bytes, &mut bitmap) {
            return None;
        }
        let highest = (0..in_group as usize)
            .rev()
            .find(|&bit| bitmap[bit / 8] & (1 << (bit % 8)) != 0);
        if let Some(bit) = highest {
            last_used = Some(group_start + bit as u64);
            break;
        }
    }
    let used_end = last_used.map_or(first_data_block, |block| block + 1) * block_bytes;
    let tail = partition_bytes.saturating_sub(used_end);
    let reserve = reserve_of(free_blocks.saturating_mul(block_bytes));
    Some(Ok(tail.saturating_sub(reserve) / (1 << 20) * (1 << 20)))
}

const NTFS_VOLUME_RECORD: u64 = 3;
const NTFS_BITMAP_RECORD: u64 = 6;
const NTFS_VOLUME_INFORMATION: u32 = 0x70;
const NTFS_DATA: u32 = 0x80;
const NTFS_END: u32 = u32::MAX;
const NTFS_VOLUME_DIRTY: u16 = 1;
const NTFS_SECTOR: usize = 512;
const NTFS_MOST_RECORD_BYTES: usize = 4096;
const NTFS_CHUNK_BYTES: usize = 64 * 1024;
const NTFS_MOST_RUNS: usize = 256;
const NTFS_ROOT_RECORD: u64 = 5;
const NTFS_INDEX_ROOT: u32 = 0x90;
const NTFS_INDEX_ALLOCATION: u32 = 0xa0;
const NTFS_LAST_ENTRY: u16 = 2;
const NTFS_RECORD_NUMBER: u64 = (1 << 48) - 1;
const NTFS_MOST_INDEX_BLOCKS: u64 = 4096;
const NTFS_MOST_INDEX_BYTES: usize = 64 * 1024;
const HIBERNATION_FILE: &str = "hiberfil.sys";

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn ntfs_record(
    record: &mut [u8],
    at: u64,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
) -> Option<()> {
    if !read(at, record) || record.get(..4)? != b"FILE" {
        return None;
    }
    ntfs_fix_up(record)
}

fn ntfs_attribute(record: &[u8], kind: u32) -> Option<&[u8]> {
    ntfs_attribute_named(record, kind, false)
}

fn ntfs_attribute_named(record: &[u8], kind: u32, named: bool) -> Option<&[u8]> {
    let mut at = usize::from(le16(record, 0x14)?);
    loop {
        let found = le32(record, at)?;
        if found == NTFS_END {
            return None;
        }
        let length = le32(record, at + 4)? as usize;
        if length == 0 {
            return None;
        }
        let unnamed = *record.get(at + 9)? == 0;
        if found == kind && unnamed != named {
            return record.get(at..at + length);
        }
        at += length;
    }
}

fn ntfs_runs(attribute: &[u8]) -> Option<Vec<(u64, u64)>> {
    let mut at = usize::from(le16(attribute, 0x20)?);
    let mut runs = Vec::new();
    let mut cluster: i64 = 0;
    while let Some(&header) = attribute.get(at) {
        if header == 0 || runs.len() == NTFS_MOST_RUNS {
            break;
        }
        let (length_bytes, offset_bytes) = (usize::from(header & 0xf), usize::from(header >> 4));
        let field = |start: usize, bytes: usize, signed: bool| -> Option<i64> {
            let raw = attribute.get(start..start + bytes)?;
            let mut value = 0i64;
            for (index, &byte) in raw.iter().enumerate() {
                value |= i64::from(byte) << (8 * index);
            }
            if signed && bytes > 0 && bytes < 8 && raw[bytes - 1] & 0x80 != 0 {
                value -= 1i64 << (8 * bytes);
            }
            Some(value)
        };
        let length = field(at + 1, length_bytes, false)?;
        if offset_bytes == 0 {
            return None;
        }
        cluster += field(at + 1 + length_bytes, offset_bytes, true)?;
        runs.push((u64::try_from(cluster).ok()?, u64::try_from(length).ok()?));
        at += 1 + length_bytes + offset_bytes;
    }
    Some(runs)
}

fn ntfs_index_find(entries: &[u8], wanted: &str) -> Option<Option<u64>> {
    let mut at = 0;
    while at + 16 <= entries.len() {
        let length = usize::from(le16(entries, at + 8)?);
        let flags = le16(entries, at + 12)?;
        if flags & NTFS_LAST_ENTRY != 0 || length == 0 {
            return Some(None);
        }
        let key = at + 16;
        let name_units = usize::from(*entries.get(key + 0x40)?);
        let name: Vec<u16> = (0..name_units)
            .filter_map(|index| le16(entries, key + 0x42 + index * 2))
            .collect();
        let matches = char::decode_utf16(name.iter().copied())
            .map(|character| character.map(|character| character.to_ascii_lowercase()))
            .eq(wanted.chars().map(Ok::<char, core::char::DecodeUtf16Error>));
        if matches {
            return Some(Some(le64(entries, at)? & NTFS_RECORD_NUMBER));
        }
        at += length;
    }
    Some(None)
}

fn ntfs_root_entry(
    mft: u64,
    record_bytes: usize,
    cluster_bytes: u64,
    wanted: &str,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
) -> Option<Option<u64>> {
    let mut record = [0u8; NTFS_MOST_RECORD_BYTES];
    let record = &mut record[..record_bytes];
    ntfs_record(record, mft + NTFS_ROOT_RECORD * record_bytes as u64, read)?;
    let root = ntfs_attribute_named(record, NTFS_INDEX_ROOT, true)?;
    let content = usize::from(le16(root, 0x14)?);
    let block_bytes = le32(root, content + 0x08)? as usize;
    let header = content + 0x10;
    let first = header + le32(root, header)? as usize;
    let end = header + le32(root, header + 4)? as usize;
    if let Some(found) = ntfs_index_find(root.get(first..end.min(root.len()))?, wanted)? {
        return Some(Some(found));
    }
    let Some(allocation) = ntfs_attribute_named(record, NTFS_INDEX_ALLOCATION, true) else {
        return Some(None);
    };
    if *allocation.get(8)? == 0 || block_bytes < NTFS_SECTOR || block_bytes > NTFS_MOST_INDEX_BYTES
    {
        return None;
    }
    let runs = ntfs_runs(allocation)?;
    let mut block = Vec::new();
    block.resize(block_bytes, 0);
    let mut blocks_read = 0;
    for (first_cluster, clusters) in runs {
        let run_bytes = clusters * cluster_bytes;
        let mut done = 0;
        while done + block_bytes as u64 <= run_bytes && blocks_read < NTFS_MOST_INDEX_BLOCKS {
            blocks_read += 1;
            if !read(first_cluster * cluster_bytes + done, &mut block) {
                return None;
            }
            done += block_bytes as u64;
            if block.get(..4) != Some(b"INDX") || ntfs_fix_up(&mut block).is_none() {
                continue;
            }
            let header = 0x18;
            let first = header + le32(&block, header)? as usize;
            let end = (header + le32(&block, header + 4)? as usize).min(block.len());
            if let Some(found) = ntfs_index_find(block.get(first..end)?, wanted)? {
                return Some(Some(found));
            }
        }
    }
    Some(None)
}

fn ntfs_fix_up(record: &mut [u8]) -> Option<()> {
    let sequence_at = usize::from(le16(record, 4)?);
    let sequence_count = usize::from(le16(record, 6)?);
    let check = le16(record, sequence_at)?;
    for sector in 1..sequence_count {
        let end = sector * NTFS_SECTOR - 2;
        if le16(record, end)? != check {
            return None;
        }
        let fixed = le16(record, sequence_at + sector * 2)?;
        record.get_mut(end..end + 2)?.copy_from_slice(&fixed.to_le_bytes());
    }
    Some(())
}

fn ntfs_hibernated(
    mft: u64,
    record_bytes: usize,
    cluster_bytes: u64,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
) -> Option<bool> {
    let Some(number) = ntfs_root_entry(mft, record_bytes, cluster_bytes, HIBERNATION_FILE, read)?
    else {
        return Some(false);
    };
    let mut record = [0u8; NTFS_MOST_RECORD_BYTES];
    let record = &mut record[..record_bytes];
    ntfs_record(record, mft + number * record_bytes as u64, read)?;
    let data = ntfs_attribute(record, NTFS_DATA)?;
    if *data.get(8)? == 0 {
        return Some(false);
    }
    let Some(&(first_cluster, _)) = ntfs_runs(data)?.first() else {
        return Some(false);
    };
    let mut magic = [0u8; 4];
    if !read(first_cluster * cluster_bytes, &mut magic) {
        return None;
    }
    Some(magic.eq_ignore_ascii_case(b"hibr"))
}

/// How much of an NTFS partition of `partition_bytes` can be taken from its end without moving
/// a file (D638): the space after the last cluster its `$Bitmap` marks used, less the reserve
/// its free space keeps; `needs-check` when `$Volume` says the volume is dirty; None where the
/// volume cannot be read.
pub fn ntfs_tail(
    start: &[u8],
    partition_bytes: u64,
    read: &mut dyn FnMut(u64, &mut [u8]) -> bool,
) -> Option<Result<u64, ShrinkBlocked>> {
    let sector_bytes = u64::from(le16(start, 0x0b)?);
    let raw_per_cluster = *start.get(0x0d)?;
    let cluster_bytes = match raw_per_cluster {
        0 => return None,
        raw if raw > 0x80 => sector_bytes.checked_shl(256 - u32::from(raw))?,
        raw => sector_bytes * u64::from(raw),
    };
    let clusters = le64(start, 0x28)?.checked_mul(sector_bytes)? / cluster_bytes.max(1);
    let mft = le64(start, 0x30)?.checked_mul(cluster_bytes)?;
    let raw_record = *start.get(0x40)? as i8;
    let record_bytes = match raw_record {
        raw if raw > 0 => u64::from(raw as u8) * cluster_bytes,
        raw => 1u64.checked_shl(u32::from(raw.unsigned_abs()))?,
    } as usize;
    if record_bytes < NTFS_SECTOR || record_bytes > NTFS_MOST_RECORD_BYTES {
        return None;
    }
    let mut record = [0u8; NTFS_MOST_RECORD_BYTES];
    let record = &mut record[..record_bytes];
    ntfs_record(record, mft + NTFS_VOLUME_RECORD * record_bytes as u64, read)?;
    let volume = ntfs_attribute(record, NTFS_VOLUME_INFORMATION)?;
    let content = usize::from(le16(volume, 0x14)?);
    if le16(volume, content + 0x0a)? & NTFS_VOLUME_DIRTY != 0 {
        return Some(Err(ShrinkBlocked::NeedsCheck));
    }
    if ntfs_hibernated(mft, record_bytes, cluster_bytes, read)? {
        return Some(Err(ShrinkBlocked::Hibernated));
    }
    ntfs_record(record, mft + NTFS_BITMAP_RECORD * record_bytes as u64, read)?;
    let data = ntfs_attribute(record, NTFS_DATA)?;
    if *data.get(8)? == 0 {
        return None;
    }
    let runs = ntfs_runs(data)?;
    let bitmap_bytes = clusters.div_ceil(8);
    let mut chunk = Vec::new();
    chunk.resize(NTFS_CHUNK_BYTES, 0);
    let (mut used, mut last_used) = (0u64, None);
    let mut file_at = 0u64;
    for (first_cluster, run_clusters) in runs {
        let run_bytes = run_clusters * cluster_bytes;
        let mut done = 0u64;
        while done < run_bytes && file_at + done < bitmap_bytes {
            let length = (run_bytes - done)
                .min(NTFS_CHUNK_BYTES as u64)
                .min(bitmap_bytes - file_at - done) as usize;
            let piece = &mut chunk[..length];
            if !read(first_cluster * cluster_bytes + done, piece) {
                return None;
            }
            for (index, &byte) in piece.iter().enumerate() {
                let first_bit = (file_at + done + index as u64) * 8;
                let byte = match clusters.saturating_sub(first_bit) {
                    0 => 0,
                    left if left < 8 => byte & ((1u8 << left) - 1),
                    _ => byte,
                };
                if byte != 0 {
                    used += u64::from(byte.count_ones());
                    last_used = Some(first_bit + 7 - u64::from(byte.leading_zeros()));
                }
            }
            done += length as u64;
        }
        file_at += run_bytes;
    }
    let used_end = last_used.map_or(0, |cluster| cluster + 1) * cluster_bytes;
    let free = clusters.saturating_sub(used) * cluster_bytes;
    let tail = partition_bytes.saturating_sub(used_end);
    Some(Ok(tail.saturating_sub(reserve_of(free)) / (1 << 20) * (1 << 20)))
}

const FAT_DIRECTORY_ENTRY: usize = 32;
const FAT_LONG_NAME: u8 = 0x0f;
const FAT_DIRECTORY: u8 = 0x10;
const FAT_VOLUME_LABEL: u8 = 0x08;
const FAT_DELETED: u8 = 0xe5;
const FAT_MOST_CLUSTERS: usize = 256;
const FAT_LONG_NAME_UNITS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

#[derive(Debug, Clone, Copy)]
struct FatVolume {
    sector_bytes: u64,
    cluster_bytes: u64,
    fat_at: u64,
    data_at: u64,
    root_at: u64,
    root_bytes: u64,
    root_cluster: u32,
    wide: bool,
}

impl FatVolume {
    fn read(boot: &[u8]) -> Option<Self> {
        let sector_bytes = u64::from(le16(boot, 0x0b)?);
        let per_cluster = u64::from(*boot.get(0x0d)?);
        let reserved = u64::from(le16(boot, 0x0e)?);
        let fats = u64::from(*boot.get(0x10)?);
        let root_entries = u64::from(le16(boot, 0x11)?);
        let small_fat = u64::from(le16(boot, 0x16)?);
        let fat_sectors = match small_fat {
            0 => u64::from(le32(boot, 0x24)?),
            sectors => sectors,
        };
        if sector_bytes == 0 || per_cluster == 0 || fats == 0 {
            return None;
        }
        let fat_at = reserved * sector_bytes;
        let root_at = fat_at + fats * fat_sectors * sector_bytes;
        let root_bytes = root_entries * FAT_DIRECTORY_ENTRY as u64;
        Some(Self {
            sector_bytes,
            cluster_bytes: per_cluster * sector_bytes,
            fat_at,
            data_at: root_at + root_bytes.next_multiple_of(sector_bytes),
            root_at,
            root_bytes,
            root_cluster: match root_entries {
                0 => le32(boot, 0x2c)?,
                _ => 0,
            },
            wide: root_entries == 0,
        })
    }

    fn next_cluster(&self, cluster: u32, read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> Option<u32> {
        let (at, bytes) = match self.wide {
            true => (u64::from(cluster) * 4, 4),
            false => (u64::from(cluster) * 2, 2),
        };
        let mut entry = [0u8; 4];
        if !read(self.fat_at + at, &mut entry[..bytes]) {
            return None;
        }
        let next = match self.wide {
            true => u32::from_le_bytes(entry) & 0x0fff_ffff,
            false => u32::from(u16::from_le_bytes([entry[0], entry[1]])),
        };
        let end = match self.wide {
            true => 0x0fff_fff8,
            false => 0xfff8,
        };
        (next >= 2 && next < end).then_some(next)
    }

    fn folder(&self, cluster: u32, read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        if cluster == 0 {
            bytes.resize(self.root_bytes as usize, 0);
            return read(self.root_at, &mut bytes).then_some(bytes);
        }
        let mut here = Some(cluster);
        let mut chunk = Vec::new();
        chunk.resize(self.cluster_bytes as usize, 0);
        for _ in 0..FAT_MOST_CLUSTERS {
            let Some(cluster) = here else {
                break;
            };
            let at = self.data_at + u64::from(cluster - 2) * self.cluster_bytes;
            if !read(at, &mut chunk) {
                return None;
            }
            bytes.extend_from_slice(&chunk);
            here = self.next_cluster(cluster, read);
        }
        Some(bytes)
    }
}

fn fat_entries(folder: &[u8]) -> Vec<(String, u8, u32)> {
    let mut found = Vec::new();
    let mut long: Vec<u16> = Vec::new();
    for entry in folder.chunks_exact(FAT_DIRECTORY_ENTRY) {
        match (entry[0], entry[11]) {
            (0, _) => break,
            (FAT_DELETED, _) => long.clear(),
            (order, FAT_LONG_NAME) => {
                let units: Vec<u16> = FAT_LONG_NAME_UNITS
                    .iter()
                    .map(|&at| u16::from_le_bytes([entry[at], entry[at + 1]]))
                    .take_while(|&unit| unit != 0 && unit != 0xffff)
                    .collect();
                if order & 0x40 != 0 {
                    long.clear();
                }
                let mut joined = units;
                joined.extend_from_slice(&long);
                long = joined;
            }
            (_, attributes) if attributes & FAT_VOLUME_LABEL != 0 => long.clear(),
            (_, attributes) => {
                let name = match long.is_empty() {
                    false => char::decode_utf16(long.iter().copied())
                        .filter_map(Result::ok)
                        .collect(),
                    true => {
                        let base = core::str::from_utf8(&entry[..8]).unwrap_or("").trim_end();
                        let extension = core::str::from_utf8(&entry[8..11]).unwrap_or("").trim_end();
                        match extension.is_empty() {
                            true => String::from(base),
                            false => alloc::format!("{base}.{extension}"),
                        }
                    }
                };
                let cluster = u32::from(u16::from_le_bytes([entry[20], entry[21]])) << 16
                    | u32::from(u16::from_le_bytes([entry[26], entry[27]]));
                found.push((name, attributes, cluster));
                long.clear();
            }
        }
    }
    found
}

fn fat_find<'a>(entries: &'a [(String, u8, u32)], name: &str) -> Option<&'a (String, u8, u32)> {
    entries
        .iter()
        .find(|(found, _, _)| found.eq_ignore_ascii_case(name))
}

/// What the EFI system partition read through `read` (its bytes from its start) holds:
/// `\EFI\Microsoft\Boot\bootmgfw.efi`, and a known distribution's folder under `\EFI`.
pub fn esp_findings(read: &mut dyn FnMut(u64, &mut [u8]) -> bool) -> Option<EspFindings> {
    let mut boot = [0u8; 512];
    if !read(0, &mut boot) {
        return None;
    }
    let volume = FatVolume::read(&boot)?;
    if volume.sector_bytes < 512 {
        return None;
    }
    let root = fat_entries(&volume.folder(volume.root_cluster, read)?);
    let efi = fat_find(&root, "EFI").filter(|(_, attributes, _)| attributes & FAT_DIRECTORY != 0)?;
    let efi = fat_entries(&volume.folder(efi.2, read)?);
    let distribution = DISTRIBUTIONS.iter().find_map(|&(folder, name)| {
        fat_find(&efi, folder)
            .filter(|(_, attributes, _)| attributes & FAT_DIRECTORY != 0)
            .map(|_| name)
    });
    let mut windows_boot_manager = false;
    if let Some(microsoft) = fat_find(&efi, "Microsoft") {
        let microsoft = fat_entries(&volume.folder(microsoft.2, read)?);
        if let Some(boot_folder) = fat_find(&microsoft, "Boot") {
            let boot_folder = fat_entries(&volume.folder(boot_folder.2, read)?);
            windows_boot_manager = fat_find(&boot_folder, "bootmgfw.efi").is_some();
        }
    }
    Some(EspFindings {
        windows_boot_manager,
        distribution,
    })
}

/// A partition's name from its UTF-16 code units: up to the first zero, control characters
/// and unpaired surrogates left out, so a terminal shows it as text.
pub fn label_of(units: &[u16]) -> String {
    let length = units.iter().position(|&unit| unit == 0).unwrap_or(units.len());
    char::decode_utf16(units[..length].iter().copied())
        .filter_map(Result::ok)
        .filter(|character| !character.is_control())
        .collect()
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
            label: String::new(),
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
        let systems = recognise(&mut partitions, EspFindings::default());
        assert_eq!(systems[0].system, SystemKind::Windows);
        assert_eq!(systems[0].partition, 3);
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
        layout.systems = recognise(&mut layout.partitions, EspFindings::default());
        layout.partitions[2].shrink_blocked = Some(ShrinkBlocked::Hibernated);
        assert_eq!(place(&layout), Err(AlongsideRefusal::SystemHibernated));
        let shrunk = Placement::ShrunkTail {
            partition: 3,
            shrink_bytes: 72 * GIB,
            following: None,
        };
        assert_eq!(opening_size_gib(&shrunk), 36);
        let small = Placement::ShrunkTail {
            partition: 3,
            shrink_bytes: 20 * GIB,
            following: None,
        };
        assert_eq!(opening_size_gib(&small), 16);
        let (first, _) = refusal_lines(AlongsideRefusal::SystemHibernated, &layout);
        assert_eq!(first, "Windows is hibernated or uses Fast Startup: its disk is in use.");
        layout.table = TableKind::Mbr;
        assert_eq!(place(&layout), Err(AlongsideRefusal::TableIsMbr));
    }

    #[test]
    fn alongside_sentences_name_what_is_shrunk_written_and_erased() {
        let mut partitions = windows_disk();
        partitions[2].shrinkable_bytes = Some(72 * GIB);
        let systems = recognise(&mut partitions, EspFindings::default());
        let layout = DiskLayout {
            table: TableKind::Gpt,
            partitions,
            unallocated: Vec::new(),
            systems,
        };
        let placement = place(&layout).unwrap();
        assert_eq!(size_range_gib(&placement), (1, 72));
        assert_eq!(
            alongside_dry_run_text(&layout, &placement, 36),
            "would shrink Windows (partition 3) from 100 GB to 64 GB, write an EggOS partition of 36 GB, add EggOS to the EFI system partition (partition 1) and add a boot entry"
        );
        assert_eq!(
            alongside_question("vdb", "/dev/vdb", &layout, &placement, 36),
            "Type vdb to install EggOS beside Windows on /dev/vdb (shrinks Windows to 64 GB, erases nothing): "
        );
        assert_eq!(erased_systems_text(&layout).as_deref(), Some("Windows and 3 more partitions"));
        assert_eq!(
            keep_hint(&layout, "/dev/vdb").as_deref(),
            Some("Windows is on this disk. To keep it: disk install --alongside /dev/vdb")
        );
        let free = Placement::Unallocated(FreeRun {
            first_sector: 1 << 30,
            sectors: 20 * (GIB / SECTOR_BYTES),
        });
        assert!(alongside_dry_run_text(&layout, &free, 20).starts_with("would write an EggOS partition of 20 GB in unallocated space"));
        assert_eq!(size_bytes_of("36G"), Some(36 * GIB));
        assert_eq!(size_bytes_of("1T"), Some(1024 * GIB));
        assert_eq!(size_bytes_of(&(3 * GIB).to_string()), Some(3 * GIB));
        assert_eq!(size_bytes_of("1000"), None);
        assert_eq!(size_bytes_of("12Q"), None);
        assert_eq!(size_bytes_of("0G"), None);
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
        let name: Vec<u16> = "Basic data\u{7}".encode_utf16().chain([0, 65]).collect();
        assert_eq!(label_of(&name), "Basic data");
    }

    #[test]
    fn an_ext4_tail_is_what_follows_its_last_used_block_less_the_reserve() {
        let block = 4096u64;
        let blocks = 4 * 32768u64;
        let mut start = [0u8; PROBE_BYTES];
        let superblock = &mut start[1024..];
        superblock[4..8].copy_from_slice(&(blocks as u32).to_le_bytes());
        superblock[12..16].copy_from_slice(&((blocks - 40000) as u32).to_le_bytes());
        superblock[24..28].copy_from_slice(&2u32.to_le_bytes());
        superblock[32..36].copy_from_slice(&32768u32.to_le_bytes());
        superblock[56..58].copy_from_slice(&[0x53, 0xef]);
        superblock[58..60].copy_from_slice(&1u16.to_le_bytes());
        let partition_bytes = blocks * block;
        let mut read = |at: u64, buffer: &mut [u8]| {
            buffer.fill(0);
            if at >= block && at < 2 * block {
                let group = (at - block) / 32;
                if group == 1 {
                    buffer[0..4].copy_from_slice(&100u32.to_le_bytes());
                }
                if group > 1 {
                    buffer[0x12] = 2;
                    buffer[0x0c..0x0e].copy_from_slice(&32768u16.to_le_bytes());
                }
            }
            if at == 100 * block {
                buffer[10] = 1;
            }
            true
        };
        let tail = ext4_tail(&start, partition_bytes, &mut read).unwrap().unwrap();
        let used_end = (32768 + 81) * block;
        let reserve = reserve_of((blocks - 40000) * block);
        assert_eq!(tail, (partition_bytes - used_end - reserve) / (1 << 20) * (1 << 20));
        let mut dirty = start;
        dirty[1024 + 58] = 0;
        assert_eq!(
            ext4_tail(&dirty, partition_bytes, &mut read),
            Some(Err(ShrinkBlocked::NeedsCheck))
        );
    }

    #[test]
    fn ntfs_data_runs_add_their_signed_offsets() {
        let mut attribute = [0u8; 0x40];
        attribute[0x20..0x22].copy_from_slice(&0x30u16.to_le_bytes());
        attribute[0x30..0x3a].copy_from_slice(&[0x21, 0x10, 0x00, 0x01, 0x21, 0x08, 0xf0, 0xff, 0x00, 0x00]);
        assert_eq!(ntfs_runs(&attribute), Some(vec![(256, 16), (240, 8)]));
    }

    #[test]
    fn an_index_entry_is_found_by_its_name_whatever_its_case() {
        let mut entries = vec![0u8; 0x200];
        let name: Vec<u16> = "HiberFil.sys".encode_utf16().collect();
        entries[0..8].copy_from_slice(&42u64.to_le_bytes());
        entries[8..10].copy_from_slice(&0x80u16.to_le_bytes());
        entries[16 + 0x40] = name.len() as u8;
        for (index, unit) in name.iter().enumerate() {
            let at = 16 + 0x42 + index * 2;
            entries[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        entries[0x80 + 12..0x80 + 14].copy_from_slice(&NTFS_LAST_ENTRY.to_le_bytes());
        entries[0x80 + 8..0x80 + 10].copy_from_slice(&16u16.to_le_bytes());
        assert_eq!(ntfs_index_find(&entries, HIBERNATION_FILE), Some(Some(42)));
        assert_eq!(ntfs_index_find(&entries, "pagefile.sys"), Some(None));
    }

    #[test]
    fn a_distribution_s_folder_names_linux() {
        let mut partitions = windows_disk();
        partitions.push(partition(5, 1 << 30, 1 << 20, PartitionType::LinuxData));
        let esp = EspFindings {
            windows_boot_manager: true,
            distribution: Some("Ubuntu"),
        };
        let systems = recognise(&mut partitions, esp);
        assert_eq!(joined_names(&systems), "Windows and Ubuntu");
    }

    #[test]
    fn fat_folders_join_their_long_names() {
        let mut folder = vec![0u8; 3 * FAT_DIRECTORY_ENTRY];
        let long = &mut folder[..FAT_DIRECTORY_ENTRY];
        long[0] = 0x41;
        long[11] = FAT_LONG_NAME;
        for (unit, &at) in "Microsoft".encode_utf16().chain([0]).zip(FAT_LONG_NAME_UNITS.iter()) {
            long[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        for &at in &FAT_LONG_NAME_UNITS[10..] {
            long[at..at + 2].copy_from_slice(&0xffffu16.to_le_bytes());
        }
        let short = &mut folder[FAT_DIRECTORY_ENTRY..2 * FAT_DIRECTORY_ENTRY];
        short[..11].copy_from_slice(b"MICROS~1   ");
        short[11] = FAT_DIRECTORY;
        short[26] = 7;
        let entries = fat_entries(&folder);
        assert_eq!(entries.len(), 1);
        assert!(fat_find(&entries, "microsoft").is_some_and(|entry| entry.2 == 7));
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
