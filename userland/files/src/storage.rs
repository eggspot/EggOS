//! Storage (D192, docs/ux/storage-v0.3.md): the machine's disks, and checking, repairing and
//! formatting an unmounted EggFS disk through its `/dev/vdX` file. One capability with one set
//! of words, shared by the `disk` command, the MCP tools `disk_list`, `disk_check`,
//! `disk_repair` and `disk_format`, and Settings' Storage pane (D22). The system is a
//! [`WholeDisks`], so host tests run it on disks in memory.

use crate::listing::size_text;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_eggfs::{
    self as eggfs, BLOCK_SIZE, CheckError, CheckMode, Contents, Disk, DiskCheck, DiskError,
    Finding, FormatError, FormatOptions, MAX_LABEL, Outcome, Volume,
};

/// The file listing the disks: one line a disk, `<name> <sectors> <ro|rw> <place>`.
pub const DISKS_FILE: &str = "/proc/driver/disks";
/// The disks' files, in the kernel's order.
pub const DEVICES: [&str; 4] = ["/dev/vda", "/dev/vdb", "/dev/vdc", "/dev/vdd"];
/// Bytes in a sector, the unit of [`DISKS_FILE`].
pub const SECTOR_BYTES: u64 = 512;
/// Where the host's shared folder, a FAT disk, is mounted.
pub const HOST_FOLDER: &str = "/media/host";
const DISKS_FILE_BYTES: usize = 1024;
const FAT_SIGNATURE: [u8; 2] = [0x55, 0xaa];
const FAT_NAMES: [(usize, &[u8]); 2] = [(54, b"FAT"), (82, b"FAT32")];

/// Why the system refused a disk operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Only root may open disks (`EPERM`, `EACCES`).
    NotAllowed,
    /// A file system or another transfer holds the disk (`EBUSY`).
    Busy,
    /// The disk takes no writes (`EROFS`).
    ReadOnly,
    /// Anything else (`EIO`, `ENOSPC`, ...).
    Failed,
}

/// How full a mounted file system is, as `statfs` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Its size in bytes.
    pub size_bytes: u64,
    /// Bytes in use.
    pub used_bytes: u64,
    /// Bytes an ordinary user can still use.
    pub available_bytes: u64,
}

/// What the capability needs from the system: the disks file, the disks' files and the clock.
pub trait WholeDisks {
    /// Reads [`DISKS_FILE`] into `buffer`; the bytes read.
    fn read_disks_file(&mut self, buffer: &mut [u8]) -> Result<usize, Fault>;
    /// Opens a disk's file, for writing too when `writable`; its handle.
    fn open(&mut self, device: &str, writable: bool) -> Result<usize, Fault>;
    /// Reads `buffer.len()` bytes at `offset`.
    fn read_at(&mut self, handle: usize, offset: u64, buffer: &mut [u8]) -> Result<(), Fault>;
    /// Writes `bytes` at `offset`.
    fn write_at(&mut self, handle: usize, offset: u64, bytes: &[u8]) -> Result<(), Fault>;
    /// Returns once every write is on the disk.
    fn flush(&mut self, handle: usize) -> Result<(), Fault>;
    /// Closes the handle.
    fn close(&mut self, handle: usize);
    /// How full the file system mounted at `place` is.
    fn usage(&mut self, place: &str) -> Option<Usage>;
    /// The time, in seconds since 1970, that a new file system's root is stamped with.
    fn now(&mut self) -> u64;
    /// Reads the system file at `path` (a `/proc/driver` file) into `buffer`; the bytes read.
    /// None by default.
    fn read_system_file(&mut self, _path: &str, _buffer: &mut [u8]) -> Result<usize, Fault> {
        Err(Fault::Failed)
    }
    /// Fills `bytes` with bytes unlikely to repeat, for new partition GUIDs.
    fn unique_bytes(&mut self, bytes: &mut [u8]) {
        let mut state = self.now() ^ 0x9e37_79b9_7f4a_7c15;
        for byte in bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }
    }
}

/// What a disk holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystem {
    /// EggFS.
    EggFs,
    /// FAT, as the host's shared folder.
    Fat,
    /// Nothing yet.
    Blank,
    /// Something else.
    Unknown,
}

impl FileSystem {
    /// Its word in `disk list --json` and MCP: `eggfs`, `fat`, `blank` or `unknown`.
    pub const fn word(self) -> &'static str {
        match self {
            FileSystem::EggFs => "eggfs",
            FileSystem::Fat => "fat",
            FileSystem::Blank => "blank",
            FileSystem::Unknown => "unknown",
        }
    }

    /// Its name for people: `EggFS`, `FAT`, `Blank` or `Unknown contents`.
    pub const fn name(self) -> &'static str {
        match self {
            FileSystem::EggFs => "EggFS",
            FileSystem::Fat => "FAT",
            FileSystem::Blank => "Blank",
            FileSystem::Unknown => "Unknown contents",
        }
    }
}

/// One disk, as `disk list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskRecord {
    /// Its file, `/dev/vda` to `/dev/vdd`.
    pub device: &'static str,
    /// Its size.
    pub size_bytes: u64,
    /// Whether it takes no writes.
    pub read_only: bool,
    /// What it holds.
    pub file_system: FileSystem,
    /// Its EggFS label, empty for none; `None` when it cannot be read (a mounted disk).
    pub label: Option<String>,
    /// Where it is mounted.
    pub mounted_on: Option<String>,
    /// Bytes in use, when known.
    pub used_bytes: Option<u64>,
    /// Bytes still free, when known.
    pub available_bytes: Option<u64>,
}

impl DiskRecord {
    /// Its file's short name: `vdb`.
    pub fn short_name(&self) -> &'static str {
        short_name(self.device)
    }
}

/// Why a disk could not be listed, checked, repaired or formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// No such disk is attached.
    NoSuchDisk,
    /// A file system holds it, mounted at this place.
    InUse(String),
    /// It takes no writes.
    ReadOnly,
    /// Only root may open disks.
    NotAllowed,
    /// It holds no EggFS file system.
    NotEggFs,
    /// The label is longer than [`MAX_LABEL`] bytes.
    LabelTooLong,
    /// The label holds a control character, which a terminal could take as a command.
    LabelNotPlain,
    /// The disk is smaller than 1 MiB.
    TooSmall,
    /// Another transfer has the disk.
    Busy,
    /// EggOS was not started by its UEFI loader, so there are no boot files to install (D241).
    NoBootFiles,
    /// The disk is smaller than [`MIN_INSTALL_BYTES`] (D241, docs/ux/installer-v0.4.md).
    TooSmallToInstall,
    /// It could not be read or written.
    Failed,
}

impl StorageError {
    /// The sentence the pane, `disk` and MCP show, with its period.
    pub fn sentence(&self) -> String {
        match self {
            StorageError::NoSuchDisk => String::from("No such disk is attached."),
            StorageError::InUse(place) => format!("In use at {place}: nothing was done."),
            StorageError::ReadOnly => String::from("Read-only disk: nothing was done."),
            StorageError::NotAllowed => String::from("Not allowed: only root may open disks."),
            StorageError::NotEggFs => String::from("No EggFS file system on the disk."),
            StorageError::LabelTooLong => {
                format!("The label is too long: {MAX_LABEL} bytes at most.")
            }
            StorageError::LabelNotPlain => {
                String::from("The label may not hold control characters.")
            }
            StorageError::TooSmall => String::from("The disk is smaller than 1 MiB."),
            StorageError::Busy => String::from("The disk is busy: try again in a moment."),
            StorageError::TooSmallToInstall => {
                String::from("The disk is smaller than 1 GB: EggOS needs 1 GB or more.")
            }
            StorageError::NoBootFiles => {
                String::from("EggOS was not started from its USB stick or disk, so it has nothing to install.")
            }
            StorageError::Failed => String::from("The disk could not be read or written."),
        }
    }

    fn of(fault: Fault) -> Self {
        match fault {
            Fault::NotAllowed => StorageError::NotAllowed,
            Fault::Busy => StorageError::Busy,
            Fault::ReadOnly => StorageError::ReadOnly,
            Fault::Failed => StorageError::Failed,
        }
    }
}

/// `vdb` for `/dev/vdb`.
pub fn short_name(device: &str) -> &str {
    device.rsplit('/').next().unwrap_or(device)
}

/// The device a short name or a path names, when it is one of [`DEVICES`].
pub fn device_named(name: &str) -> Option<&'static str> {
    DEVICES
        .iter()
        .copied()
        .find(|device| *device == name || short_name(device) == name)
}

struct Line {
    device: &'static str,
    sectors: u64,
    read_only: bool,
    place: Option<String>,
}

fn lines(disks: &mut (impl WholeDisks + ?Sized)) -> Result<Vec<Line>, StorageError> {
    let mut buffer = [0u8; DISKS_FILE_BYTES];
    let length = disks
        .read_disks_file(&mut buffer)
        .map_err(StorageError::of)?;
    let text = core::str::from_utf8(buffer.get(..length).unwrap_or_default()).unwrap_or("");
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_ascii_whitespace();
            let device = device_named(words.next()?)?;
            let sectors = words.next()?.parse().ok()?;
            let read_only = words.next()? == "ro";
            let place = words.next().filter(|place| *place != "-").map(String::from);
            Some(Line {
                device,
                sectors,
                read_only,
                place,
            })
        })
        .collect())
}

struct DeviceDisk<'a, W: WholeDisks + ?Sized> {
    disks: &'a mut W,
    handle: usize,
    blocks: u64,
    writable: bool,
    fault: Option<Fault>,
}

impl<W: WholeDisks + ?Sized> DeviceDisk<'_, W> {
    fn failed(&mut self, fault: Fault) -> DiskError {
        if self.fault.is_none() {
            self.fault = Some(fault);
        }
        DiskError
    }
}

impl<W: WholeDisks + ?Sized> Disk for DeviceDisk<'_, W> {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<(), DiskError> {
        let offset = block.saturating_mul(BLOCK_SIZE as u64);
        self.disks
            .read_at(self.handle, offset, buffer)
            .map_err(|fault| self.failed(fault))
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<(), DiskError> {
        if !self.writable {
            return Err(self.failed(Fault::ReadOnly));
        }
        let offset = block.saturating_mul(BLOCK_SIZE as u64);
        self.disks
            .write_at(self.handle, offset, buffer)
            .map_err(|fault| self.failed(fault))
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        match self.writable {
            true => self
                .disks
                .flush(self.handle)
                .map_err(|fault| self.failed(fault)),
            false => Ok(()),
        }
    }
}

fn blocks_of(sectors: u64) -> u64 {
    sectors.saturating_mul(SECTOR_BYTES) / BLOCK_SIZE as u64
}

fn is_fat(sector: &[u8]) -> bool {
    sector.get(510..512) == Some(&FAT_SIGNATURE[..])
        && FAT_NAMES.iter().any(|(at, name)| {
            sector
                .get(*at..at + name.len())
                .is_some_and(|found| found == *name)
        })
}

fn record(disks: &mut (impl WholeDisks + ?Sized), line: Line) -> DiskRecord {
    let mut record = DiskRecord {
        device: line.device,
        size_bytes: line.sectors.saturating_mul(SECTOR_BYTES),
        read_only: line.read_only,
        file_system: FileSystem::Unknown,
        label: None,
        mounted_on: None,
        used_bytes: None,
        available_bytes: None,
    };
    if let Some(place) = line.place {
        record.file_system = match place == HOST_FOLDER {
            true => FileSystem::Fat,
            false => FileSystem::EggFs,
        };
        if let Some(usage) = disks.usage(&place) {
            record.used_bytes = Some(usage.used_bytes);
            record.available_bytes = Some(usage.available_bytes);
        }
        record.mounted_on = Some(place);
        return record;
    }
    let Ok(handle) = disks.open(line.device, false) else {
        return record;
    };
    let mut disk = DeviceDisk {
        disks: &mut *disks,
        handle,
        blocks: blocks_of(line.sectors),
        writable: false,
        fault: None,
    };
    match eggfs::probe(&mut disk) {
        Ok(Contents::EggFs) => {
            record.file_system = FileSystem::EggFs;
            if let Ok((volume, _)) = Volume::mount(disk) {
                let (blocks, free) = volume.space();
                let block = BLOCK_SIZE as u64;
                record.label = Some(shown_label(volume.label()));
                record.used_bytes = Some(blocks.saturating_sub(free).saturating_mul(block));
                record.available_bytes = Some(free.saturating_mul(block));
            }
        }
        Ok(Contents::Blank) => record.file_system = FileSystem::Blank,
        Ok(Contents::Other) => {
            let mut sector = [0u8; SECTOR_BYTES as usize];
            if disk.disks.read_at(handle, 0, &mut sector).is_ok() && is_fat(&sector) {
                record.file_system = FileSystem::Fat;
            }
        }
        Err(DiskError) => {}
    }
    disks.close(handle);
    record
}

/// Every disk attached, in the kernel's order (`disk list`, `disk_list`, the Storage pane).
pub fn list(disks: &mut (impl WholeDisks + ?Sized)) -> Result<Vec<DiskRecord>, StorageError> {
    Ok(lines(disks)?
        .into_iter()
        .map(|line| record(disks, line))
        .collect())
}

/// One disk's record, by its device or short name.
pub fn find(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
) -> Result<DiskRecord, StorageError> {
    let wanted = device_named(device).ok_or(StorageError::NoSuchDisk)?;
    let line = lines(disks)?
        .into_iter()
        .find(|line| line.device == wanted)
        .ok_or(StorageError::NoSuchDisk)?;
    Ok(record(disks, line))
}

fn unmounted(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
    writes: bool,
) -> Result<Line, StorageError> {
    let wanted = device_named(device).ok_or(StorageError::NoSuchDisk)?;
    let line = lines(disks)?
        .into_iter()
        .find(|line| line.device == wanted)
        .ok_or(StorageError::NoSuchDisk)?;
    if let Some(place) = line.place {
        return Err(StorageError::InUse(place));
    }
    if writes && line.read_only {
        return Err(StorageError::ReadOnly);
    }
    Ok(line)
}

/// Checks the unmounted EggFS disk `device`: read-only, or repairing what is safe
/// (`disk check`, `disk repair`). A mounted disk is refused, and so is a repair of a read-only
/// one.
pub fn check(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
    mode: CheckMode,
) -> Result<DiskCheck, StorageError> {
    let writable = mode == CheckMode::Repair;
    let line = unmounted(disks, device, writable)?;
    let now = disks.now();
    let handle = disks
        .open(line.device, writable)
        .map_err(StorageError::of)?;
    let mut disk = DeviceDisk {
        disks: &mut *disks,
        handle,
        blocks: blocks_of(line.sectors),
        writable,
        fault: None,
    };
    let checked = eggfs::check_disk(&mut disk, mode, now);
    let fault = disk.fault;
    disks.close(handle);
    match checked {
        Ok(check) => Ok(check),
        Err(CheckError::NotEggFs) => Err(StorageError::NotEggFs),
        Err(CheckError::Io) => Err(fault.map_or(StorageError::Failed, StorageError::of)),
    }
}

/// Erases the unmounted disk `device` and writes an empty EggFS file system labelled `label`
/// on all of it (`disk format`).
pub fn format(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
    label: &str,
) -> Result<(), StorageError> {
    check_label(label)?;
    let line = unmounted(disks, device, true)?;
    let now = disks.now();
    let handle = disks.open(line.device, true).map_err(StorageError::of)?;
    let mut disk = DeviceDisk {
        disks: &mut *disks,
        handle,
        blocks: blocks_of(line.sectors),
        writable: true,
        fault: None,
    };
    let options = FormatOptions {
        blocks: None,
        label,
    };
    let formatted = eggfs::format_with(&mut disk, now, &options);
    let fault = disk.fault;
    disks.close(handle);
    match formatted {
        Ok(()) => Ok(()),
        Err(FormatError::TooSmall) => Err(StorageError::TooSmall),
        Err(FormatError::BadLabel) => Err(StorageError::LabelTooLong),
        Err(FormatError::Io | FormatError::LargerThanDisk) => {
            Err(fault.map_or(StorageError::Failed, StorageError::of))
        }
    }
}

/// Whether `label` fits: at most [`MAX_LABEL`] bytes, no control character.
pub fn label_fits(label: &str) -> bool {
    check_label(label).is_ok()
}

/// Why `label` cannot be a disk's label, if it cannot: longer than [`MAX_LABEL`] bytes or
/// holding a control character.
pub fn check_label(label: &str) -> Result<(), StorageError> {
    if label.chars().any(char::is_control) {
        return Err(StorageError::LabelNotPlain);
    }
    match label.len() > MAX_LABEL {
        true => Err(StorageError::LabelTooLong),
        false => Ok(()),
    }
}

fn shown_label(stored: &[u8]) -> String {
    String::from_utf8_lossy(stored)
        .chars()
        .map(|character| match character.is_control() {
            true => char::REPLACEMENT_CHARACTER,
            false => character,
        })
        .collect()
}

/// The contents line: `EggFS "Photos"`, `EggFS, no label`, `EggFS` (label unknown),
/// `FAT (the host's shared folder)`, `Blank` or `Unknown contents`.
pub fn contents_text(record: &DiskRecord) -> String {
    match (record.file_system, &record.label) {
        (FileSystem::EggFs, Some(label)) if label.is_empty() => String::from("EggFS, no label"),
        (FileSystem::EggFs, Some(label)) => format!("EggFS \"{label}\""),
        (FileSystem::Fat, _) if record.mounted_on.as_deref() == Some(HOST_FOLDER) => {
            String::from("FAT (the host's shared folder)")
        }
        (file_system, _) => String::from(file_system.name()),
    }
}

/// The size line: `1.0 GB · 312 MB used · 712 MB free`, or `1.0 GB · space unknown`.
pub fn sizes_text(record: &DiskRecord) -> String {
    let size = size_text(record.size_bytes);
    match (record.used_bytes, record.available_bytes) {
        (Some(used), Some(free)) => format!(
            "{} \u{b7} {} used \u{b7} {} free",
            size.as_str(),
            size_text(used).as_str(),
            size_text(free).as_str()
        ),
        _ => format!("{} \u{b7} space unknown", size.as_str()),
    }
}

/// The place line: `Mounted at /home` or `Not mounted`.
pub fn place_text(record: &DiskRecord) -> String {
    match &record.mounted_on {
        Some(place) => format!("Mounted at {place}"),
        None => String::from("Not mounted"),
    }
}

/// What formatting erases: `EggFS "Photos", 1.0 GB`, `Blank, 1.0 GB`.
pub fn erased_text(record: &DiskRecord) -> String {
    format!(
        "{}, {}",
        contents_text(record),
        size_text(record.size_bytes).as_str()
    )
}

/// `262,144`.
pub fn grouped(number: u64) -> String {
    let digits = format!("{number}");
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

fn problems(count: usize) -> String {
    match count {
        1 => String::from("1 problem"),
        count => format!("{count} problems"),
    }
}

/// A check's headline: `Clean. 262,144 blocks, 182,000 free.`, `Found 3 problems: 2
/// repairable, 1 not. ...` or `Repaired 2 problems; 1 not repairable. ...`.
pub fn headline(check: &DiskCheck) -> String {
    let space = format!(
        "{} blocks, {} free.",
        grouped(check.blocks),
        grouped(check.free_blocks)
    );
    let count = |outcome: Outcome| {
        check
            .findings
            .iter()
            .filter(|finding| finding.outcome == outcome)
            .count()
    };
    let not = count(Outcome::NotRepairable);
    match (check.is_clean(), check.mode) {
        (true, _) => format!("Clean. {space}"),
        (false, CheckMode::ReadOnly) => {
            let repairable = count(Outcome::Repairable);
            let rest = match not {
                0 => String::new(),
                not => format!(", {not} not"),
            };
            format!(
                "Found {}: {repairable} repairable{rest}. {space}",
                problems(check.findings.len())
            )
        }
        (false, CheckMode::Repair) => {
            let rest = match not {
                0 => String::new(),
                not => format!("; {not} not repairable"),
            };
            format!(
                "Repaired {}{rest}. {space}",
                problems(count(Outcome::Repaired))
            )
        }
    }
}

/// An outcome's word in `--json` and MCP: `repairable`, `repaired` or `not-repairable`.
pub const fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Repairable => "repairable",
        Outcome::Repaired => "repaired",
        Outcome::NotRepairable => "not-repairable",
    }
}

/// An outcome for people: `Repairable`, `Repaired` or `Not repairable`.
pub const fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Repairable => "Repairable",
        Outcome::Repaired => "Repaired",
        Outcome::NotRepairable => "Not repairable",
    }
}

/// Where a finding is: `inode 17`, `block 4,096`, `inode 17, block 4,096`, or empty.
pub fn place_of(finding: &Finding) -> String {
    match (finding.inode, finding.block) {
        (0, 0) => String::new(),
        (inode, 0) => format!("inode {inode}"),
        (0, block) => format!("block {}", grouped(u64::from(block))),
        (inode, block) => format!("inode {inode}, block {}", grouped(u64::from(block))),
    }
}

/// The pane's line for a finding: `Repairable · inode 17 · <description> (<code>)`.
pub fn finding_line(finding: &Finding) -> String {
    let place = place_of(finding);
    let middle = match place.is_empty() {
        true => String::new(),
        false => format!("{place} \u{b7} "),
    };
    format!(
        "{} \u{b7} {middle}{} ({})",
        outcome_name(finding.outcome),
        finding.problem.description(),
        finding.problem.code()
    )
}

/// The Storage pane's list when no disk is attached.
pub const NO_DISKS: &str = "No disks attached.";
/// The Storage pane's status line when no disk is attached.
pub const NOTHING_TO_DO: &str = "No disk attached: nothing to check or format.";
const CHECK_FIRST: &str = "Check reads the disk and changes nothing. Repair is enabled when a \
                           check finds something to repair.";
const READ_ONLY_EGGFS: &str = "Read-only disk: it can be checked, not repaired or formatted.";
const NO_EGGFS: &str =
    "No EggFS file system on the disk: nothing to check. Format... makes it one.";
const READ_ONLY_FOREIGN: &str =
    "Read-only disk with no EggFS file system: nothing can be done to it.";
/// The text fields' placeholders that ask for each disk's short name, in [`DEVICES`] order.
pub const CONFIRM_PLACEHOLDERS: [&str; 4] = [
    "Type vda to confirm",
    "Type vdb to confirm",
    "Type vdc to confirm",
    "Type vdd to confirm",
];

/// What may be done to a disk: checked, repaired, formatted. The pane enables `Repair` only
/// after a check of the same disk found something repairable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowed {
    /// It is an unmounted EggFS disk.
    pub check: bool,
    /// It is an unmounted EggFS disk that takes writes.
    pub repair: bool,
    /// It is unmounted and takes writes.
    pub format: bool,
}

/// What may be done to `record`.
pub fn allowed(record: &DiskRecord) -> Allowed {
    let unmounted = record.mounted_on.is_none();
    let eggfs = record.file_system == FileSystem::EggFs;
    Allowed {
        check: unmounted && eggfs,
        repair: unmounted && eggfs && !record.read_only,
        format: unmounted && !record.read_only,
    }
}

/// The pane's status line while nothing runs: why a button is disabled, or what Check does.
pub fn idle_text(record: &DiskRecord) -> String {
    let eggfs = record.file_system == FileSystem::EggFs;
    match (&record.mounted_on, eggfs, record.read_only) {
        (Some(place), _, _) => format!(
            "In use at {place}: a mounted disk cannot be checked, repaired or formatted in this \
             version."
        ),
        (None, true, false) => String::from(CHECK_FIRST),
        (None, true, true) => String::from(READ_ONLY_EGGFS),
        (None, false, false) => String::from(NO_EGGFS),
        (None, false, true) => String::from(READ_ONLY_FOREIGN),
    }
}

/// `Checking /dev/vdb...` or `Repairing /dev/vdb...`.
pub fn running_text(device: &str, mode: CheckMode) -> String {
    match mode {
        CheckMode::ReadOnly => format!("Checking {device}..."),
        CheckMode::Repair => format!("Repairing {device}..."),
    }
}

/// `Formatting /dev/vdb...`.
pub fn formatting_text(device: &str) -> String {
    format!("Formatting {device}...")
}

/// `Formatted /dev/vdb as EggFS "Photos".` or `Formatted /dev/vdb as EggFS, no label.`
pub fn formatted_text(device: &str, label: &str) -> String {
    match label.is_empty() {
        true => format!("Formatted {device} as EggFS, no label."),
        false => format!("Formatted {device} as EggFS \"{label}\"."),
    }
}

/// The format question: `Format /dev/vdb?`.
pub fn format_question(device: &str) -> String {
    format!("Format {device}?")
}

/// `Erases everything on it: EggFS "Photos", 1.0 GB.`
pub fn erases_text(record: &DiskRecord) -> String {
    format!("Erases everything on it: {}.", erased_text(record))
}

/// `This cannot be undone. Type vdb to confirm.`
pub fn confirm_text(record: &DiskRecord) -> String {
    format!(
        "This cannot be undone. Type {} to confirm.",
        record.short_name()
    )
}

/// The placeholder of the field that asks for `device`'s short name.
pub fn confirm_placeholder(device: &str) -> &'static str {
    DEVICES
        .iter()
        .position(|known| *known == device)
        .map_or(CONFIRM_PLACEHOLDERS[0], |index| CONFIRM_PLACEHOLDERS[index])
}

/// The last report line when a check found more than the pane shows: `and 4 more: disk check
/// /dev/vdb lists them all.`
pub fn more_findings_text(more: usize, device: &str) -> String {
    format!("and {more} more: disk check {device} lists them all.")
}

/// The share of the disk in use, in thousandths, when its use is known.
pub fn used_permille(record: &DiskRecord) -> Option<u16> {
    let used = record.used_bytes?;
    if record.size_bytes == 0 {
        return None;
    }
    let permille = u128::from(used) * 1000 / u128::from(record.size_bytes);
    Some(permille.min(1000) as u16)
}

/// Disks in memory, for host tests of every front-end.
#[cfg(any(test, feature = "memory"))]
pub mod memory {
    use super::{Fault, SECTOR_BYTES, Usage, WholeDisks};
    use eggos_eggfs::memory::MemoryDisk;
    use eggos_eggfs::{BLOCK_SIZE, FormatOptions, format_with};

    /// A disk in memory.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct MemoryWholeDisk {
        /// Its short name, `vda`.
        pub name: &'static str,
        /// Its bytes.
        pub image: Vec<u8>,
        /// Whether it takes no writes.
        pub read_only: bool,
        /// Where it is mounted.
        pub mounted_on: Option<&'static str>,
        /// What `statfs` says of it when mounted.
        pub usage: Option<Usage>,
        /// What every open of it fails with.
        pub refused: Option<Fault>,
        /// What every read of it fails with.
        pub broken: Option<Fault>,
        /// Writes made to it.
        pub writes: u64,
    }

    impl MemoryWholeDisk {
        /// A disk of `blocks` blocks of zeros.
        pub fn blank(name: &'static str, blocks: usize) -> Self {
            Self {
                name,
                image: vec![0; blocks * BLOCK_SIZE],
                read_only: false,
                mounted_on: None,
                usage: None,
                refused: None,
                broken: None,
                writes: 0,
            }
        }

        /// A disk of `blocks` blocks holding an empty EggFS file system labelled `label`.
        pub fn eggfs(name: &'static str, blocks: usize, label: &str) -> Self {
            let mut disk = MemoryDisk::new(blocks);
            let options = FormatOptions {
                blocks: None,
                label,
            };
            format_with(&mut disk, 0, &options).expect("formats");
            Self {
                image: disk.image().to_vec(),
                ..Self::blank(name, 0)
            }
        }
    }

    /// The machine's disks in memory.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct MemoryDisks {
        /// The disks, in the kernel's order.
        pub disks: Vec<MemoryWholeDisk>,
        /// Handles open now.
        pub open: Vec<usize>,
    }

    impl MemoryDisks {
        /// These disks.
        pub fn of(disks: Vec<MemoryWholeDisk>) -> Self {
            Self {
                disks,
                open: Vec::new(),
            }
        }

        /// The disk named `name`.
        pub fn disk(&self, name: &str) -> &MemoryWholeDisk {
            self.disks
                .iter()
                .find(|disk| disk.name == name)
                .expect("a disk")
        }

        fn range(
            &self,
            handle: usize,
            offset: u64,
            length: usize,
        ) -> Result<(usize, usize), Fault> {
            let disk = self.disks.get(handle).ok_or(Fault::Failed)?;
            if let Some(fault) = disk.broken {
                return Err(fault);
            }
            let start = usize::try_from(offset).map_err(|_| Fault::Failed)?;
            let end = start.checked_add(length).ok_or(Fault::Failed)?;
            match end <= disk.image.len() {
                true => Ok((start, end)),
                false => Err(Fault::Failed),
            }
        }
    }

    impl WholeDisks for MemoryDisks {
        fn read_disks_file(&mut self, buffer: &mut [u8]) -> Result<usize, Fault> {
            let mut text = String::new();
            for disk in &self.disks {
                let sectors = disk.image.len() as u64 / SECTOR_BYTES;
                let access = if disk.read_only { "ro" } else { "rw" };
                let place = disk.mounted_on.unwrap_or("-");
                text.push_str(&format!("{} {sectors} {access} {place}\n", disk.name));
            }
            let length = text.len().min(buffer.len());
            buffer[..length].copy_from_slice(&text.as_bytes()[..length]);
            Ok(length)
        }

        fn open(&mut self, device: &str, writable: bool) -> Result<usize, Fault> {
            let name = super::short_name(device);
            let handle = self
                .disks
                .iter()
                .position(|disk| disk.name == name)
                .ok_or(Fault::Failed)?;
            let disk = &self.disks[handle];
            if let Some(fault) = disk.refused {
                return Err(fault);
            }
            if writable && disk.mounted_on.is_some() {
                return Err(Fault::Busy);
            }
            if writable && disk.read_only {
                return Err(Fault::ReadOnly);
            }
            self.open.push(handle);
            Ok(handle)
        }

        fn read_at(&mut self, handle: usize, offset: u64, buffer: &mut [u8]) -> Result<(), Fault> {
            let (start, end) = self.range(handle, offset, buffer.len())?;
            buffer.copy_from_slice(&self.disks[handle].image[start..end]);
            Ok(())
        }

        fn write_at(&mut self, handle: usize, offset: u64, bytes: &[u8]) -> Result<(), Fault> {
            let (start, end) = self.range(handle, offset, bytes.len())?;
            let disk = &mut self.disks[handle];
            disk.image[start..end].copy_from_slice(bytes);
            disk.writes += 1;
            Ok(())
        }

        fn flush(&mut self, _handle: usize) -> Result<(), Fault> {
            Ok(())
        }

        fn close(&mut self, handle: usize) {
            if let Some(at) = self.open.iter().position(|open| *open == handle) {
                self.open.remove(at);
            }
        }

        fn usage(&mut self, place: &str) -> Option<Usage> {
            self.disks
                .iter()
                .find(|disk| disk.mounted_on == Some(place))
                .and_then(|disk| disk.usage)
        }

        fn now(&mut self) -> u64 {
            1_800_000_000
        }
    }
}

#[cfg(test)]
mod tests {
    use super::memory::{MemoryDisks, MemoryWholeDisk};
    use super::*;

    const BLOCKS: usize = 512;

    fn machine() -> MemoryDisks {
        let mut home = MemoryWholeDisk::eggfs("vda", BLOCKS, "Home");
        home.mounted_on = Some("/home");
        home.usage = Some(Usage {
            size_bytes: 2 << 20,
            used_bytes: 1 << 20,
            available_bytes: 1 << 20,
        });
        MemoryDisks::of(vec![
            home,
            MemoryWholeDisk::eggfs("vdb", BLOCKS, "Photos"),
            MemoryWholeDisk::blank("vdc", BLOCKS),
        ])
    }

    fn damage_the_block_bitmap(disks: &mut MemoryDisks, name: &str) {
        let disk = disks
            .disks
            .iter_mut()
            .find(|disk| disk.name == name)
            .unwrap();
        disk.image[BLOCK_SIZE..2 * BLOCK_SIZE].fill(0);
    }

    #[test]
    fn the_list_has_each_disk_s_contents_label_place_and_space_and_opens_nothing_for_writing() {
        let mut disks = machine();
        let records = list(&mut disks).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].device, "/dev/vda");
        assert_eq!(contents_text(&records[0]), "EggFS");
        assert_eq!(place_text(&records[0]), "Mounted at /home");
        assert_eq!(
            sizes_text(&records[0]),
            "2.0 MB \u{b7} 1.0 MB used \u{b7} 1.0 MB free"
        );
        assert_eq!(contents_text(&records[1]), "EggFS \"Photos\"");
        assert_eq!(records[1].mounted_on, None);
        assert!(records[1].used_bytes.unwrap() > 0);
        assert_eq!(
            records[1].used_bytes.unwrap() + records[1].available_bytes.unwrap(),
            records[1].size_bytes
        );
        assert_eq!(records[2].file_system, FileSystem::Blank);
        assert_eq!(sizes_text(&records[2]), "2.0 MB \u{b7} space unknown");
        assert_eq!(erased_text(&records[2]), "Blank, 2.0 MB");
        assert!(disks.open.is_empty());
        assert!(disks.disks.iter().all(|disk| disk.writes == 0));
    }

    #[test]
    fn a_clean_disk_checks_clean_and_a_damaged_one_is_repaired_then_clean() {
        let mut disks = machine();
        let clean = check(&mut disks, "/dev/vdb", CheckMode::ReadOnly).unwrap();
        assert!(clean.is_clean());
        assert!(headline(&clean).starts_with("Clean. 512 blocks, "));
        damage_the_block_bitmap(&mut disks, "vdb");
        let found = check(&mut disks, "vdb", CheckMode::ReadOnly).unwrap();
        assert!(!found.is_clean() && found.is_repairable());
        assert!(
            headline(&found).starts_with("Found "),
            "{}",
            headline(&found)
        );
        assert!(finding_line(&found.findings[0]).starts_with("Repairable \u{b7} "));
        assert_eq!(disks.disk("vdb").writes, 0);
        let repaired = check(&mut disks, "/dev/vdb", CheckMode::Repair).unwrap();
        assert!(headline(&repaired).starts_with("Repaired "));
        assert!(disks.disk("vdb").writes > 0);
        assert!(
            check(&mut disks, "/dev/vdb", CheckMode::ReadOnly)
                .unwrap()
                .is_clean()
        );
        assert!(disks.open.is_empty());
    }

    #[test]
    fn mounted_read_only_missing_and_foreign_disks_are_refused() {
        let mut disks = machine();
        assert_eq!(
            check(&mut disks, "/dev/vda", CheckMode::ReadOnly),
            Err(StorageError::InUse(String::from("/home")))
        );
        assert_eq!(
            format(&mut disks, "vda", ""),
            Err(StorageError::InUse(String::from("/home")))
        );
        assert_eq!(
            check(&mut disks, "/dev/vdc", CheckMode::ReadOnly),
            Err(StorageError::NotEggFs)
        );
        assert_eq!(
            check(&mut disks, "/dev/vdd", CheckMode::ReadOnly),
            Err(StorageError::NoSuchDisk)
        );
        assert_eq!(
            check(&mut disks, "/etc/passwd", CheckMode::ReadOnly),
            Err(StorageError::NoSuchDisk)
        );
        disks.disks[1].read_only = true;
        assert_eq!(
            check(&mut disks, "/dev/vdb", CheckMode::Repair),
            Err(StorageError::ReadOnly)
        );
        assert!(check(&mut disks, "/dev/vdb", CheckMode::ReadOnly).is_ok());
        assert_eq!(
            format(&mut disks, "/dev/vdb", ""),
            Err(StorageError::ReadOnly)
        );
        disks.disks[1].read_only = false;
        disks.disks[1].refused = Some(Fault::NotAllowed);
        assert_eq!(
            check(&mut disks, "/dev/vdb", CheckMode::ReadOnly),
            Err(StorageError::NotAllowed)
        );
        disks.disks[1].refused = None;
        disks.disks[1].broken = Some(Fault::Failed);
        assert_eq!(
            check(&mut disks, "/dev/vdb", CheckMode::ReadOnly),
            Err(StorageError::Failed)
        );
        assert_eq!(
            StorageError::Failed.sentence(),
            "The disk could not be read or written."
        );
    }

    #[test]
    fn formatting_makes_an_empty_labelled_eggfs_and_refuses_a_long_label_first() {
        let mut disks = machine();
        let long = "x".repeat(MAX_LABEL + 1);
        assert_eq!(
            format(&mut disks, "/dev/vdc", &long),
            Err(StorageError::LabelTooLong)
        );
        assert_eq!(disks.disk("vdc").writes, 0);
        assert_eq!(
            format(&mut disks, "/dev/vdc", "a\u{1b}[2Jb"),
            Err(StorageError::LabelNotPlain)
        );
        assert_eq!(disks.disk("vdc").writes, 0);
        assert!(!label_fits("tab\there") && label_fits("Fotos \u{e9}t\u{e9}"));
        assert_eq!(shown_label(b"a\x1b[31mb\x85"), "a\u{fffd}[31mb\u{fffd}");
        format(&mut disks, "/dev/vdc", "scratch").unwrap();
        let record = find(&mut disks, "vdc").unwrap();
        assert_eq!(contents_text(&record), "EggFS \"scratch\"");
        assert!(
            check(&mut disks, "/dev/vdc", CheckMode::ReadOnly)
                .unwrap()
                .is_clean()
        );
        format(&mut disks, "/dev/vdb", "").unwrap();
        assert_eq!(
            contents_text(&find(&mut disks, "vdb").unwrap()),
            "EggFS, no label"
        );
        let mut tiny = MemoryDisks::of(vec![MemoryWholeDisk::blank("vda", 16)]);
        assert_eq!(format(&mut tiny, "vda", ""), Err(StorageError::TooSmall));
    }

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(262_144), "262,144");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn each_disk_says_what_may_be_done_to_it_and_why_not() {
        let mut disks = machine();
        disks.disks.push(MemoryWholeDisk {
            read_only: true,
            ..MemoryWholeDisk::eggfs("vdd", BLOCKS, "Old")
        });
        let records = list(&mut disks).unwrap();
        let allowed_of = |index: usize| {
            let allowed = allowed(&records[index]);
            (allowed.check, allowed.repair, allowed.format)
        };
        assert_eq!(allowed_of(0), (false, false, false));
        assert_eq!(allowed_of(1), (true, true, true));
        assert_eq!(allowed_of(2), (false, false, true));
        assert_eq!(allowed_of(3), (true, false, false));
        assert_eq!(
            idle_text(&records[0]),
            "In use at /home: a mounted disk cannot be checked, repaired or formatted in this \
             version."
        );
        assert!(idle_text(&records[1]).starts_with("Check reads the disk and changes nothing."));
        assert!(idle_text(&records[2]).ends_with("Format... makes it one."));
        assert!(idle_text(&records[3]).starts_with("Read-only disk: it can be checked"));
        assert_eq!(used_permille(&records[0]), Some(500));
        assert_eq!(used_permille(&records[2]), None);
        assert_eq!(
            erases_text(&records[2]),
            "Erases everything on it: Blank, 2.0 MB."
        );
        assert_eq!(
            confirm_text(&records[2]),
            "This cannot be undone. Type vdc to confirm."
        );
        assert_eq!(confirm_placeholder("/dev/vdc"), "Type vdc to confirm");
        assert_eq!(
            formatted_text("/dev/vdc", "Photos"),
            "Formatted /dev/vdc as EggFS \"Photos\"."
        );
        assert_eq!(
            formatted_text("/dev/vdc", ""),
            "Formatted /dev/vdc as EggFS, no label."
        );
        assert_eq!(
            running_text("/dev/vdb", CheckMode::Repair),
            "Repairing /dev/vdb..."
        );
        assert_eq!(
            more_findings_text(4, "/dev/vdb"),
            "and 4 more: disk check /dev/vdb lists them all."
        );
    }
}

/// `/proc/driver/boot` and the boot files it sizes (D241).
pub const BOOT_FILE: &str = "/proc/driver/boot";
pub const BOOT_KERNEL_FILE: &str = "/proc/driver/boot-kernel";
pub const BOOT_LOADER_FILE: &str = "/proc/driver/boot-loader";
/// The EFI system partition an installation gets.
pub const SYSTEM_PARTITION_BYTES: u64 = 256 << 20;
/// The smallest disk EggOS installs on.
pub const MIN_INSTALL_BYTES: u64 = 1 << 30;
const MAX_BOOT_FILE_BYTES: usize = 256 << 20;
const BOOT_TEXT_BYTES: usize = 256;
const DATA_START_ZEROED: u64 = 1 << 20;
const ZERO_CHUNK: usize = 64 * 1024;

/// The files EggOS was started from: the loader and the kernel, and the loader's name on the
/// EFI system partition, `BOOTX64.EFI` or `BOOTAA64.EFI`.
#[derive(Debug)]
pub struct BootFiles {
    pub loader: Vec<u8>,
    pub kernel: Vec<u8>,
    pub loader_name: &'static str,
}

fn size_named(text: &str, name: &str) -> Option<usize> {
    text.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')?.trim().parse().ok())
}

fn read_whole(disks: &mut (impl WholeDisks + ?Sized), path: &str, bytes: usize) -> Result<Vec<u8>, StorageError> {
    if bytes == 0 || bytes > MAX_BOOT_FILE_BYTES {
        return Err(StorageError::NoBootFiles);
    }
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(bytes).map_err(|_| StorageError::Failed)?;
    buffer.resize(bytes, 0);
    let read = disks
        .read_system_file(path, &mut buffer)
        .map_err(|_| StorageError::NoBootFiles)?;
    if read != bytes {
        return Err(StorageError::NoBootFiles);
    }
    Ok(buffer)
}

/// The PE machine of a UEFI application: its file name on the EFI system partition.
pub fn loader_name_of(loader: &[u8]) -> Option<&'static str> {
    let header = usize::from(u16::from_le_bytes([*loader.get(0x3c)?, *loader.get(0x3d)?]));
    let machine = u16::from_le_bytes([*loader.get(header + 4)?, *loader.get(header + 5)?]);
    match machine {
        0x8664 => Some("BOOTX64.EFI"),
        0xaa64 => Some("BOOTAA64.EFI"),
        _ => None,
    }
}

/// Reads the boot files the kernel kept.
pub fn boot_files(disks: &mut (impl WholeDisks + ?Sized)) -> Result<BootFiles, StorageError> {
    let mut text = [0u8; BOOT_TEXT_BYTES];
    let length = disks
        .read_system_file(BOOT_FILE, &mut text)
        .map_err(|_| StorageError::NoBootFiles)?;
    let text = core::str::from_utf8(&text[..length]).map_err(|_| StorageError::NoBootFiles)?;
    let kernel_bytes = size_named(text, "kernel").ok_or(StorageError::NoBootFiles)?;
    let loader_bytes = size_named(text, "loader").ok_or(StorageError::NoBootFiles)?;
    let loader = read_whole(disks, BOOT_LOADER_FILE, loader_bytes)?;
    let kernel = read_whole(disks, BOOT_KERNEL_FILE, kernel_bytes)?;
    let loader_name = loader_name_of(&loader).ok_or(StorageError::NoBootFiles)?;
    Ok(BootFiles {
        loader,
        kernel,
        loader_name,
    })
}

/// What an installation put on its disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installation {
    pub device: &'static str,
    pub system_partition_bytes: u64,
    pub data_partition_bytes: u64,
}

/// Where an installation's partitions go on a disk of `sectors` 512-byte sectors: the EFI system
/// partition's first and last sector and the data partition's.
pub fn installation_layout(sectors: u64) -> Option<(eggos_gpt::Layout, [(u64, u64); 2])> {
    if sectors.saturating_mul(512) < MIN_INSTALL_BYTES {
        return None;
    }
    let layout = eggos_gpt::Layout::new(sectors, 512)?;
    let system_first = layout.aligned(layout.first_usable);
    let system_last = system_first + SYSTEM_PARTITION_BYTES / 512 - 1;
    let data_first = layout.aligned(system_last + 1);
    (data_first < layout.last_usable).then_some((layout, [(system_first, system_last), (data_first, layout.last_usable)]))
}

/// Installs EggOS on the unmounted disk `device` (`disk install`, D241): erases it, writes a GUID
/// partition table with an EFI system partition holding `files` and an EggOS data partition,
/// whose start is zeroed so the next boot from the disk makes it an empty EggFS volume.
pub fn install(
    disks: &mut (impl WholeDisks + ?Sized),
    device: &str,
    files: &BootFiles,
) -> Result<Installation, StorageError> {
    let line = unmounted(disks, device, true)?;
    let (layout, [(system_first, system_last), (data_first, data_last)]) =
        installation_layout(line.sectors).ok_or(StorageError::TooSmallToInstall)?;
    let mut unique = [0u8; 52];
    disks.unique_bytes(&mut unique);
    let guid = |at: usize| {
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&unique[at..at + 16]);
        eggos_gpt::random_guid(bytes)
    };
    let volume_id = u32::from_le_bytes([unique[48], unique[49], unique[50], unique[51]]);
    let partitions = [
        eggos_gpt::NewPartition {
            kind: eggos_gpt::EFI_SYSTEM_PARTITION,
            unique: guid(16),
            first: system_first,
            last: system_last,
            name: "EFI system partition",
        },
        eggos_gpt::NewPartition {
            kind: eggos_gpt::EGGOS_DATA_PARTITION,
            unique: guid(32),
            first: data_first,
            last: data_last,
            name: "EggOS",
        },
    ];
    let table = eggos_gpt::NewTable::new(layout, guid(0), &partitions).ok_or(StorageError::TooSmall)?;
    let handle = disks.open(line.device, true).map_err(StorageError::of)?;
    let result = write_installation(disks, handle, &table, files, system_first, system_last, data_first, volume_id);
    let flushed = disks.flush(handle).map_err(StorageError::of);
    disks.close(handle);
    result?;
    flushed?;
    Ok(Installation {
        device: line.device,
        system_partition_bytes: (system_last - system_first + 1) * 512,
        data_partition_bytes: (data_last - data_first + 1) * 512,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_installation(
    disks: &mut (impl WholeDisks + ?Sized),
    handle: usize,
    table: &eggos_gpt::NewTable,
    files: &BootFiles,
    system_first: u64,
    system_last: u64,
    data_first: u64,
    volume_id: u32,
) -> Result<(), StorageError> {
    let [primary_header, primary_entries, backup_entries, backup_header] = table.places();
    let mut header_block = [0u8; 512];
    disks.write_at(handle, 0, &table.mbr).map_err(StorageError::of)?;
    header_block[..eggos_gpt::HEADER_BYTES].copy_from_slice(&table.primary_header);
    disks.write_at(handle, primary_header * 512, &header_block).map_err(StorageError::of)?;
    disks.write_at(handle, primary_entries * 512, &table.entries).map_err(StorageError::of)?;
    disks.write_at(handle, backup_entries * 512, &table.entries).map_err(StorageError::of)?;
    header_block[..eggos_gpt::HEADER_BYTES].copy_from_slice(&table.backup_header);
    disks.write_at(handle, backup_header * 512, &header_block).map_err(StorageError::of)?;
    let loader_path = format!("EFI/BOOT/{}", files.loader_name);
    let volume_files = [
        eggos_fat32::File {
            path: &loader_path,
            bytes: &files.loader,
        },
        eggos_fat32::File {
            path: "EGGOS/KERNEL.ELF",
            bytes: &files.kernel,
        },
    ];
    let sectors = u32::try_from(system_last - system_first + 1).map_err(|_| StorageError::TooSmall)?;
    let hidden = u32::try_from(system_first).unwrap_or(u32::MAX);
    let base = system_first * 512;
    eggos_fat32::write_volume(sectors, hidden, volume_id, &volume_files, &mut |offset, bytes| {
        disks.write_at(handle, base + offset, bytes)
    })
    .map_err(|error| match error {
        eggos_fat32::Error::Write(fault) => StorageError::of(fault),
        _ => StorageError::TooSmall,
    })?;
    let zero = [0u8; ZERO_CHUNK];
    let mut offset = 0;
    while offset < DATA_START_ZEROED {
        disks.write_at(handle, data_first * 512 + offset, &zero).map_err(StorageError::of)?;
        offset += ZERO_CHUNK as u64;
    }
    Ok(())
}

/// Whether `/proc/driver/boot`'s text names both kept boot files with a size (D241).
pub fn boot_text_has_files(text: &str) -> bool {
    [size_named(text, "kernel"), size_named(text, "loader")]
        .iter()
        .all(|size| size.is_some_and(|size| size > 0))
}

/// Whether the kernel kept the files EggOS was started from, which installing needs.
pub fn has_boot_files(disks: &mut (impl WholeDisks + ?Sized)) -> bool {
    let mut text = [0u8; BOOT_TEXT_BYTES];
    let Ok(length) = disks.read_system_file(BOOT_FILE, &mut text) else {
        return false;
    };
    core::str::from_utf8(&text[..length]).is_ok_and(boot_text_has_files)
}

/// Whether EggOS may be installed on `record`, checked in `disk install`'s order: mounted,
/// read-only, too small, no boot files.
pub fn install_allowed(record: &DiskRecord, boot_files: bool) -> Result<(), StorageError> {
    if let Some(place) = &record.mounted_on {
        return Err(StorageError::InUse(place.clone()));
    }
    if record.read_only {
        return Err(StorageError::ReadOnly);
    }
    if installation_layout(record.size_bytes / 512).is_none() {
        return Err(StorageError::TooSmallToInstall);
    }
    if !boot_files {
        return Err(StorageError::NoBootFiles);
    }
    Ok(())
}

/// The Storage pane's install line for `record` while nothing runs (docs/ux/installer-v0.4.md).
pub fn install_idle_text(record: &DiskRecord, boot_files: bool) -> String {
    match install_allowed(record, boot_files) {
        Ok(()) => String::from(
            "Install EggOS... erases the disk and writes EggOS on it, to start this machine from.",
        ),
        Err(StorageError::InUse(place)) => {
            format!("In use at {place}: EggOS cannot be installed on a mounted disk.")
        }
        Err(StorageError::ReadOnly) => String::from("Read-only disk: EggOS cannot be installed on it."),
        Err(StorageError::TooSmallToInstall) => {
            String::from("Smaller than 1 GB: EggOS needs a disk of 1 GB or more.")
        }
        Err(error) => error.sentence(),
    }
}

/// The install line with no disk attached but boot files kept.
pub const NO_DISK_TO_INSTALL_ON: &str =
    "EggOS sees no disk to install on. It reads NVMe and SATA disks, not USB ones.";

/// The install line after an installation, until EggOS starts from the disk.
pub const INSTALLED_NOTE: &str = "It reads Unknown contents here until EggOS is started from it.";

/// `Install EggOS on /dev/vdb?`
pub fn install_question(device: &str) -> String {
    format!("Install EggOS on {device}?")
}

/// The two partitions an installation on `record` writes, as the install panel says them.
pub fn install_writes_lines(record: &DiskRecord) -> [String; 2] {
    let data_bytes = installation_layout(record.size_bytes / 512)
        .map_or(0, |(_, [_, (first, last)])| (last - first + 1) * 512);
    [
        format!(
            "Writes an EFI system partition, {},",
            size_text(SYSTEM_PARTITION_BYTES).as_str()
        ),
        format!(
            "and an EggOS partition, {}, for your files.",
            size_text(data_bytes).as_str()
        ),
    ]
}

/// `Installing EggOS on /dev/vdb... This can take a minute.`
pub fn installing_text(device: &str) -> String {
    format!("Installing EggOS on {device}... This can take a minute.")
}

/// `disk install`'s done line, without its newline.
pub fn installed_text(device: &str) -> String {
    format!("Installed EggOS on {device}. Restart and choose it in the firmware's boot menu.")
}

#[cfg(test)]
mod install_tests {
    use super::*;

    fn record(size_bytes: u64, mounted_on: Option<&str>, read_only: bool) -> DiskRecord {
        DiskRecord {
            device: "/dev/vdb",
            size_bytes,
            read_only,
            file_system: FileSystem::Blank,
            label: None,
            mounted_on: mounted_on.map(String::from),
            used_bytes: None,
            available_bytes: None,
        }
    }

    #[test]
    fn installing_is_refused_in_the_cli_s_order() {
        let gib = 1 << 30;
        assert_eq!(install_allowed(&record(gib, None, false), true), Ok(()));
        assert_eq!(
            install_allowed(&record(gib, Some("/home"), true), true),
            Err(StorageError::InUse(String::from("/home")))
        );
        assert_eq!(install_allowed(&record(gib, None, true), true), Err(StorageError::ReadOnly));
        assert_eq!(
            install_allowed(&record(512 << 20, None, false), true),
            Err(StorageError::TooSmallToInstall)
        );
        assert_eq!(install_allowed(&record(gib, None, false), false), Err(StorageError::NoBootFiles));
    }

    #[test]
    fn the_boot_text_needs_both_files() {
        assert!(boot_text_has_files("partition 00\nkernel 100\nloader 20\n"));
        assert!(!boot_text_has_files("kernel 100\n"));
        assert!(!boot_text_has_files("kernel 0\nloader 20\n"));
    }

    #[test]
    fn the_layout_leaves_a_256_mib_system_partition_and_the_rest() {
        let (_, [(first, last), (data_first, _)]) = installation_layout((2u64 << 30) / 512).unwrap();
        assert_eq!(first, 2048);
        assert_eq!(last - first + 1, 524_288);
        assert!(data_first > last);
    }
}
