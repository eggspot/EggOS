//! `cargo xtask work` and `run --work`: a host folder carried into EggOS as a writable disk
//! mounted at `/work`, and what EggOS wrote there carried back out (D166, CI stage 1).
//!
//! [`pack`] writes the folder into a disk image with EggOS's own file system (EggFS, the
//! `eggos-eggfs` crate), marked as a work disk so EggOS mounts it at `/work` and not at
//! `/home`; [`unpack`] reads the image after QEMU has ended and writes `/work`'s tree into a
//! new folder on the host. The image is a sparse file sized to the folder plus room for what
//! builds write; files keep their permission bits (on Unix hosts) and modification times.
//! Symbolic links keep their targets and hard links stay one file with several names, both
//! ways (Windows gives every file one name, and may refuse to make symbolic links to a user
//! without the privilege: those are reported and left out). The folder's top-level `.git` and
//! `target` are left out, as is anything that is neither a file, a folder nor a symbolic link.

use eggos_eggfs::{BLOCK_SIZE, Disk, DiskError, Kind, MAX_LINK_TARGET, MAX_NAME, Volume};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The top-level entries of a folder that are not packed: its Git history and its build
/// output, which a build inside EggOS makes again.
pub const SKIPPED_AT_TOP: [&str; 2] = [".git", "target"];

/// Room a work disk has beyond what the folder holds, for what EggOS writes to it.
pub const HEADROOM_BYTES: u64 = 1 << 30;

const MEBIBYTE: u64 = 1 << 20;

const COPY_CHUNK_BYTES: usize = 1 << 20;

const MAX_VOLUME_BLOCKS: u64 = u32::MAX as u64 - 1 - 32768;

/// A disk image on the host as an EggFS disk. Flushes do nothing: the image is a scratch file
/// that [`pack`] syncs once at its end and [`unpack`] only reads back.
#[derive(Debug)]
pub struct FileDisk {
    file: File,
    blocks: u64,
}

impl FileDisk {
    /// The image at `path`, opened for reading and writing.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes = file
            .metadata()
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        Ok(Self {
            file,
            blocks: bytes / BLOCK_SIZE as u64,
        })
    }

    fn seek_to(&mut self, block: u64, bytes: usize) -> Result<(), DiskError> {
        let blocks = (bytes / BLOCK_SIZE) as u64;
        if !bytes.is_multiple_of(BLOCK_SIZE)
            || block
                .checked_add(blocks)
                .is_none_or(|end| end > self.blocks)
        {
            return Err(DiskError);
        }
        self.file
            .seek(SeekFrom::Start(block * BLOCK_SIZE as u64))
            .map(|_| ())
            .map_err(|_| DiskError)
    }
}

impl Disk for FileDisk {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<(), DiskError> {
        self.seek_to(block, buffer.len())?;
        self.file.read_exact(buffer).map_err(|_| DiskError)
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<(), DiskError> {
        self.seek_to(block, buffer.len())?;
        self.file.write_all(buffer).map_err(|_| DiskError)
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        Ok(())
    }
}

/// What [`pack`] put on the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Packed {
    /// Regular files packed.
    pub files: u64,
    /// Folders packed, the top one not counted.
    pub folders: u64,
    /// Bytes of the files packed.
    pub bytes: u64,
    /// Symbolic links packed.
    pub symbolic_links: u64,
    /// Names packed as further names of a file already packed.
    pub hard_links: u64,
    /// Special files, and symbolic links EggFS cannot hold, left out.
    pub skipped: u64,
    /// The image's size in bytes.
    pub image_bytes: u64,
}

/// What [`unpack`] wrote on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unpacked {
    /// Regular files written.
    pub files: u64,
    /// Folders made, the top one not counted.
    pub folders: u64,
    /// Bytes of the files written.
    pub bytes: u64,
    /// Symbolic links made.
    pub symbolic_links: u64,
    /// Names made as hard links to a file already written.
    pub hard_links: u64,
    /// Symbolic links the host refused to make, left out.
    pub skipped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ItemKind {
    Folder,
    File {
        bytes: u64,
        identity: Option<(u64, u64)>,
    },
    Link {
        target: Vec<u8>,
    },
}

#[derive(Debug, Clone)]
struct Item {
    parts: Vec<String>,
    host: PathBuf,
    kind: ItemKind,
    permissions: u32,
    modified: u64,
}

#[derive(Debug, Default)]
struct Scan {
    items: Vec<Item>,
    skipped: Vec<(PathBuf, String)>,
}

fn seconds(time: std::io::Result<SystemTime>) -> u64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs())
}

#[cfg(unix)]
fn permissions_of(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn permissions_of(metadata: &std::fs::Metadata) -> u32 {
    match (metadata.is_dir(), metadata.permissions().readonly()) {
        (true, _) => 0o755,
        (false, true) => 0o444,
        (false, false) => 0o644,
    }
}

#[cfg(unix)]
fn identity_of(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    if metadata.nlink() > 1 {
        Some((metadata.dev(), metadata.ino()))
    } else {
        None
    }
}

#[cfg(not(unix))]
fn identity_of(_: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[cfg(unix)]
fn link_target_bytes(target: &Path) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    Some(target.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn link_target_bytes(target: &Path) -> Option<Vec<u8>> {
    target
        .to_str()
        .map(|text| text.replace('\\', "/").into_bytes())
}

fn link_kind(host: &Path) -> Result<Result<ItemKind, String>, String> {
    let target = std::fs::read_link(host).map_err(|e| format!("{}: {e}", host.display()))?;
    let Some(target) = link_target_bytes(&target) else {
        let reason = "a symbolic link whose target is not valid Unicode";
        return Ok(Err(reason.to_owned()));
    };
    if target.is_empty() || target.len() > MAX_LINK_TARGET || target.contains(&0) {
        let reason =
            format!("a symbolic link whose target is empty or longer than {MAX_LINK_TARGET} bytes");
        return Ok(Err(reason));
    }
    Ok(Ok(ItemKind::Link { target }))
}

fn scan_folder(folder: &Path, parts: &[String], scan: &mut Scan) -> Result<(), String> {
    let mut entries = std::fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .map(|entry| entry.map_err(|e| format!("{}: {e}", folder.display())))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let host = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("{}: the name is not valid Unicode", host.display()))?;
        if parts.is_empty() && SKIPPED_AT_TOP.contains(&name.as_str()) {
            continue;
        }
        if name.len() > MAX_NAME {
            return Err(format!(
                "{}: the name is longer than {MAX_NAME} bytes",
                host.display()
            ));
        }
        let metadata =
            std::fs::symlink_metadata(&host).map_err(|e| format!("{}: {e}", host.display()))?;
        let kind = if metadata.is_dir() {
            ItemKind::Folder
        } else if metadata.is_file() {
            ItemKind::File {
                bytes: metadata.len(),
                identity: identity_of(&metadata),
            }
        } else if metadata.file_type().is_symlink() {
            match link_kind(&host)? {
                Ok(kind) => kind,
                Err(reason) => {
                    scan.skipped.push((host, reason));
                    continue;
                }
            }
        } else {
            scan.skipped
                .push((host, "not a file, a folder or a symbolic link".to_owned()));
            continue;
        };
        let mut item_parts = parts.to_vec();
        item_parts.push(name);
        let is_folder = kind == ItemKind::Folder;
        scan.items.push(Item {
            parts: item_parts.clone(),
            host: host.clone(),
            kind,
            permissions: permissions_of(&metadata),
            modified: seconds(metadata.modified()),
        });
        if is_folder {
            scan_folder(&host, &item_parts, scan)?;
        }
    }
    Ok(())
}

fn blocks_for(bytes: u64) -> u64 {
    bytes.div_ceil(BLOCK_SIZE as u64)
}

/// The size of a work disk for `items` files and folders holding `bytes`: the blocks they
/// take with a thirty-second more for pointer blocks and directories, plus `room` bytes
/// ([`HEADROOM_BYTES`] for [`pack`]), and at least eight blocks per item so that half the
/// inodes stay free (EggFS has one inode per four blocks), in whole mebibytes.
pub fn image_bytes(bytes: u64, items: u64, room: u64) -> u64 {
    let content = blocks_for(bytes) * BLOCK_SIZE as u64;
    let wanted = content + content / 32 + room;
    let for_inodes = (items + 64) * 2 * 4 * BLOCK_SIZE as u64;
    wanted.max(for_inodes).div_ceil(MEBIBYTE) * MEBIBYTE
}

fn now() -> u64 {
    seconds(Ok(SystemTime::now()))
}

fn eggfs_error(what: &str, error: eggos_eggfs::Error) -> String {
    let reason = match error {
        eggos_eggfs::Error::NoSpace => "the work disk is full",
        eggos_eggfs::Error::NameTooLong => "the name is too long",
        eggos_eggfs::Error::TooBig => "the file is larger than EggFS allows",
        eggos_eggfs::Error::Corrupt => "the work disk is damaged",
        eggos_eggfs::Error::Io => "the image could not be read or written",
        eggos_eggfs::Error::TooManyLinks => "the file has as many names as EggFS allows",
        _ => "EggFS refused it",
    };
    format!("{what}: {reason} ({error:?})")
}

fn item_name(item: &Item) -> &[u8] {
    item.parts.last().map_or("", String::as_str).as_bytes()
}

fn pack_item(
    volume: &mut Volume<FileDisk>,
    parent: u64,
    item: &Item,
    buffer: &mut [u8],
) -> Result<u64, String> {
    let shown = item.host.display().to_string();
    let name = item_name(item);
    let made = match &item.kind {
        ItemKind::Folder => volume.create(
            parent,
            name,
            Kind::Directory,
            item.permissions,
            item.modified,
        ),
        ItemKind::File { .. } => {
            volume.create(parent, name, Kind::Regular, item.permissions, item.modified)
        }
        ItemKind::Link { target } => volume.symlink(parent, name, target, item.modified),
    };
    let node = made.map_err(|error| eggfs_error(&shown, error))?;
    if let ItemKind::File { .. } = item.kind {
        let mut file = File::open(&item.host).map_err(|e| format!("{shown}: {e}"))?;
        let mut offset = 0u64;
        loop {
            let read = file.read(buffer).map_err(|e| format!("{shown}: {e}"))?;
            if read == 0 {
                break;
            }
            let (written, after) = volume
                .write(node, Some(offset), &buffer[..read], item.modified)
                .map_err(|error| eggfs_error(&shown, error))?;
            if written < read {
                return Err(eggfs_error(&shown, eggos_eggfs::Error::NoSpace));
            }
            offset = after;
        }
        volume
            .set_modified(node, item.modified)
            .map_err(|error| eggfs_error(&shown, error))?;
    }
    Ok(node)
}

/// Writes `folder` (without its top-level [`SKIPPED_AT_TOP`] entries and special files) into a
/// new work disk image at `image` with [`HEADROOM_BYTES`] to spare, replacing any file there,
/// and says what it packed. Each skipped path is printed as a warning with its reason.
pub fn pack(folder: &Path, image: &Path) -> Result<Packed, String> {
    pack_with_room(folder, image, HEADROOM_BYTES)
}

fn pack_with_room(folder: &Path, image: &Path, room: u64) -> Result<Packed, String> {
    if !folder.is_dir() {
        return Err(format!("`{}` is not a folder", folder.display()));
    }
    let mut scan = Scan::default();
    scan_folder(folder, &[], &mut scan)?;
    for (skipped, reason) in &scan.skipped {
        eprintln!(
            "xtask: {}: {reason}: left out of the work disk",
            skipped.display()
        );
    }
    let bytes: u64 = scan
        .items
        .iter()
        .map(|item| match item.kind {
            ItemKind::File { bytes, .. } => bytes,
            ItemKind::Folder | ItemKind::Link { .. } => BLOCK_SIZE as u64,
        })
        .sum();
    let image_bytes = image_bytes(bytes, scan.items.len() as u64, room);
    if image_bytes / BLOCK_SIZE as u64 > MAX_VOLUME_BLOCKS {
        return Err(format!(
            "`{}` holds more than an EggFS volume can",
            folder.display()
        ));
    }
    if let Some(parent) = image
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let file = File::create(image).map_err(|e| format!("{}: {e}", image.display()))?;
    file.set_len(image_bytes)
        .map_err(|e| format!("{}: {e}", image.display()))?;
    drop(file);
    let mut disk = FileDisk::open(image)?;
    let made = now();
    eggos_eggfs::format(&mut disk, made)
        .map_err(|error| format!("{}: could not format it: {error:?}", image.display()))?;
    let (mut volume, _) = Volume::mount(disk)
        .map_err(|(error, _)| format!("{}: {}", image.display(), error.description()))?;
    let top = eggos_eggfs::make_work_disk(&mut volume, made)
        .map_err(|error| eggfs_error(&image.display().to_string(), error))?;
    let mut nodes: HashMap<&[String], u64> = HashMap::new();
    let mut first_names: HashMap<(u64, u64), u64> = HashMap::new();
    let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
    let mut packed = Packed {
        skipped: scan.skipped.len() as u64,
        image_bytes,
        ..Packed::default()
    };
    for item in &scan.items {
        let parent_parts = &item.parts[..item.parts.len() - 1];
        let parent = match parent_parts.is_empty() {
            true => top,
            false => *nodes
                .get(parent_parts)
                .ok_or_else(|| format!("{}: its folder was not packed", item.host.display()))?,
        };
        let linked = match &item.kind {
            ItemKind::File {
                identity: Some(identity),
                ..
            } => first_names.get(identity).copied(),
            _ => None,
        };
        if let Some(node) = linked {
            volume
                .link(node, parent, item_name(item), item.modified)
                .map_err(|error| eggfs_error(&item.host.display().to_string(), error))?;
            packed.hard_links += 1;
            continue;
        }
        let node = pack_item(&mut volume, parent, item, &mut buffer)?;
        match &item.kind {
            ItemKind::Folder => {
                packed.folders += 1;
                nodes.insert(&item.parts, node);
            }
            ItemKind::File { identity, .. } => {
                packed.files += 1;
                packed.bytes += volume
                    .attributes(node)
                    .map_err(|error| eggfs_error(&item.host.display().to_string(), error))?
                    .size;
                if let Some(identity) = identity {
                    first_names.insert(*identity, node);
                }
            }
            ItemKind::Link { .. } => packed.symbolic_links += 1,
        }
    }
    for item in scan.items.iter().rev() {
        if let (ItemKind::Folder, Some(&node)) = (&item.kind, nodes.get(item.parts.as_slice())) {
            volume
                .set_modified(node, item.modified)
                .map_err(|error| eggfs_error(&item.host.display().to_string(), error))?;
        }
    }
    volume
        .commit()
        .map_err(|error| eggfs_error(&image.display().to_string(), error))?;
    let disk = volume.into_disk();
    disk.file
        .sync_all()
        .map_err(|e| format!("{}: {e}", image.display()))?;
    Ok(packed)
}

fn host_name(name: &[u8], within: &Path) -> Result<String, String> {
    let text = std::str::from_utf8(name).map_err(|_| {
        format!(
            "{}: a name EggOS wrote there is not valid Unicode",
            within.display()
        )
    })?;
    let mut components = Path::new(text).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(part)), None) if part == text => Ok(text.to_owned()),
        _ => Err(format!(
            "{}: EggOS wrote `{text}`, which is not a name this host allows",
            within.display()
        )),
    }
}

fn is_empty_folder(path: &Path) -> Result<bool, String> {
    Ok(std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .next()
        .is_none())
}

fn write_file(
    volume: &mut Volume<FileDisk>,
    node: u64,
    path: &Path,
    buffer: &mut [u8],
) -> Result<u64, String> {
    let shown = path.display().to_string();
    let attributes = volume
        .attributes(node)
        .map_err(|error| eggfs_error(&shown, error))?;
    let mut file = File::create(path).map_err(|e| format!("{shown}: {e}"))?;
    let mut offset = 0u64;
    loop {
        let read = volume
            .read(node, offset, buffer)
            .map_err(|error| eggfs_error(&shown, error))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("{shown}: {e}"))?;
        offset += read as u64;
    }
    let modified = UNIX_EPOCH + Duration::from_secs(attributes.modified);
    file.set_modified(modified)
        .map_err(|e| format!("{shown}: {e}"))?;
    drop(file);
    set_permissions(path, attributes.permissions)?;
    Ok(offset)
}

#[cfg(unix)]
fn set_permissions(path: &Path, permissions: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(permissions & 0o7777))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_permissions(path: &Path, permissions: u32) -> Result<(), String> {
    if permissions & 0o222 != 0 {
        return Ok(());
    }
    let mut host = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .permissions();
    host.set_readonly(true);
    std::fs::set_permissions(path, host).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn make_host_link(
    _: &mut Volume<FileDisk>,
    _: u64,
    target: &[u8],
    path: &Path,
) -> Result<Option<String>, String> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(target), path)
        .map(|()| None)
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(windows)]
const PRIVILEGE_NOT_HELD: i32 = 1314;

#[cfg(windows)]
fn names_directory(volume: &mut Volume<FileDisk>, directory: u64, target: &[u8]) -> bool {
    if target.starts_with(b"/") {
        return false;
    }
    let mut node = directory;
    for part in target.split(|&byte| byte == b'/') {
        node = match part {
            b"" | b"." => node,
            b".." => match volume.parent(node) {
                Ok(parent) => parent,
                Err(_) => return false,
            },
            name => match volume.lookup(node, name) {
                Ok(Some(found)) => found,
                _ => return false,
            },
        };
    }
    eggos_eggfs::kind_of(node) == Kind::Directory
}

#[cfg(windows)]
fn make_host_link(
    volume: &mut Volume<FileDisk>,
    directory: u64,
    target: &[u8],
    path: &Path,
) -> Result<Option<String>, String> {
    let Ok(text) = std::str::from_utf8(target) else {
        return Ok(Some("its target is not valid Unicode".to_owned()));
    };
    let host_target = text.replace('/', "\\");
    let made = match names_directory(volume, directory, target) {
        true => std::os::windows::fs::symlink_dir(&host_target, path),
        false => std::os::windows::fs::symlink_file(&host_target, path),
    };
    match made {
        Ok(()) => Ok(None),
        Err(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(PRIVILEGE_NOT_HELD) =>
        {
            let reason = format!("Windows refused to make the symbolic link ({error})");
            Ok(Some(reason))
        }
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Writes the tree under `/work` on the work disk image `image` into the folder `out`, which
/// must not exist or be empty, and says what it wrote. The image's journal is replayed first,
/// so a QEMU that was killed still gives back everything EggOS committed, and the whole file
/// system is checked before anything is written. A file with several names is written once
/// and its other names made as hard links; a symbolic link the host refuses to make is
/// printed as a warning and left out.
pub fn unpack(image: &Path, out: &Path) -> Result<Unpacked, String> {
    if out.exists() && !(out.is_dir() && is_empty_folder(out)?) {
        return Err(format!(
            "`{}` is not an empty folder: unpack into a new one",
            out.display()
        ));
    }
    let disk = FileDisk::open(image)?;
    let (mut volume, _) = Volume::mount(disk)
        .map_err(|(error, _)| format!("{}: {}", image.display(), error.description()))?;
    volume.check().map_err(|problem| {
        format!(
            "{}: the work disk is inconsistent: {problem}",
            image.display()
        )
    })?;
    let top = eggos_eggfs::work_folder(&mut volume).map_err(|_| {
        format!(
            "{}: not a work disk (no {} marker and work folder)",
            image.display(),
            String::from_utf8_lossy(eggos_eggfs::WORK_DISK_MARKER)
        )
    })?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut unpacked = Unpacked::default();
    let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
    let mut first_names: HashMap<u64, PathBuf> = HashMap::new();
    let mut pending = vec![(out.to_path_buf(), top)];
    while let Some((folder, directory)) = pending.pop() {
        let mut position = 0;
        while let Some(entry) = volume
            .entry(directory, position)
            .map_err(|error| eggfs_error(&folder.display().to_string(), error))?
        {
            position += 1;
            let path = folder.join(host_name(entry.name(), &folder)?);
            match eggos_eggfs::kind_of(entry.node) {
                Kind::Directory => {
                    std::fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    unpacked.folders += 1;
                    pending.push((path, entry.node));
                }
                Kind::Regular => {
                    if let Some(first) = first_names.get(&entry.node) {
                        std::fs::hard_link(first, &path)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        unpacked.hard_links += 1;
                        continue;
                    }
                    unpacked.bytes += write_file(&mut volume, entry.node, &path, &mut buffer)?;
                    unpacked.files += 1;
                    let links = volume
                        .attributes(entry.node)
                        .map_err(|error| eggfs_error(&path.display().to_string(), error))?
                        .links;
                    if links > 1 {
                        first_names.insert(entry.node, path);
                    }
                }
                Kind::SymbolicLink => {
                    let target = volume
                        .link_target(entry.node)
                        .map_err(|error| eggfs_error(&path.display().to_string(), error))?;
                    match make_host_link(&mut volume, directory, &target, &path)? {
                        None => unpacked.symbolic_links += 1,
                        Some(reason) => {
                            eprintln!(
                                "xtask: {}: {reason}: left out of the unpacked folder",
                                path.display()
                            );
                            unpacked.skipped += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(unpacked)
}

/// The folder `run --work <folder>` unpacks into when no `--work-out` names one: a new
/// folder beside it named `<folder>.out`.
pub fn default_output(folder: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let name = absolute
        .components()
        .next_back()
        .and_then(|last| match last {
            Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "`{}` names no folder to put results beside",
                folder.display()
            )
        })?;
    let mut out = name;
    out.push(".out");
    Ok(absolute.with_file_name(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ROOM: u64 = 8 << 20;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eggos-work-disk-test-{name}-{}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn tree(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut out = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.is_dir() {
                    out.push((relative, None));
                    pending.push(path);
                } else {
                    out.push((relative, Some(std::fs::read(&path).unwrap())));
                }
            }
        }
        out.sort();
        out
    }

    fn pattern(length: usize) -> Vec<u8> {
        (0..length).map(|at| (at * 7 % 251) as u8).collect()
    }

    #[test]
    fn a_folder_packs_and_unpacks_to_the_same_tree_without_git_and_target() {
        let root = scratch("round-trip");
        let folder = root.join("repo");
        write(&folder.join("Cargo.toml"), b"[workspace]\n");
        write(&folder.join("src/main.rs"), b"fn main() {}\n");
        write(
            &folder.join("src/deep/data.bin"),
            &pattern(3 * COPY_CHUNK_BYTES + 17),
        );
        write(&folder.join("empty.txt"), b"");
        std::fs::create_dir_all(folder.join("docs/empty")).unwrap();
        write(&folder.join(".git/HEAD"), b"ref: refs/heads/main\n");
        write(&folder.join("target/debug/big"), b"build output");
        write(&folder.join("src/target/kept.rs"), b"kept\n");
        let image = root.join("work.img");
        let packed = pack_with_room(&folder, &image, TEST_ROOM).unwrap();
        assert_eq!(packed.files, 5);
        assert_eq!(packed.folders, 5);
        assert_eq!(packed.skipped, 0);
        assert_eq!(
            packed.bytes,
            (3 * COPY_CHUNK_BYTES + 17 + 12 + 13 + 5) as u64
        );
        assert_eq!(std::fs::metadata(&image).unwrap().len(), packed.image_bytes);
        let out = root.join("out");
        let unpacked = unpack(&image, &out).unwrap();
        assert_eq!(
            (unpacked.files, unpacked.folders, unpacked.bytes),
            (packed.files, packed.folders, packed.bytes)
        );
        let mut expected = tree(&folder);
        expected.retain(|(path, _)| !path.starts_with(".git") && !path.starts_with("target"));
        assert_eq!(tree(&out), expected);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn modification_times_come_back_and_the_image_is_a_marked_work_disk() {
        let root = scratch("times");
        let folder = root.join("repo");
        write(&folder.join("a.txt"), b"a");
        let file = File::options()
            .write(true)
            .open(folder.join("a.txt"))
            .unwrap();
        let then = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        file.set_modified(then).unwrap();
        drop(file);
        let image = root.join("work.img");
        pack_with_room(&folder, &image, TEST_ROOM).unwrap();
        let (mut volume, _) = Volume::mount(FileDisk::open(&image).unwrap()).unwrap();
        assert_eq!(eggos_eggfs::is_work_disk(&mut volume), Ok(true));
        let top = eggos_eggfs::work_folder(&mut volume).unwrap();
        let node = volume.lookup(top, b"a.txt").unwrap().unwrap();
        assert_eq!(volume.attributes(node).unwrap().modified, 1_700_000_000);
        volume.check().unwrap();
        drop(volume);
        let out = root.join("out");
        unpack(&image, &out).unwrap();
        let modified = std::fs::metadata(out.join("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(modified, then);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn permission_bits_symbolic_links_and_hard_links_survive_a_round_trip() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root = scratch("unix");
        let folder = root.join("repo");
        write(&folder.join("run.sh"), b"#!/bin/eggsh\necho hi\n");
        std::fs::set_permissions(
            folder.join("run.sh"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        std::os::unix::fs::symlink("run.sh", folder.join("link")).unwrap();
        std::os::unix::fs::symlink("../outside/missing", folder.join("dangling")).unwrap();
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::os::unix::fs::symlink("..", folder.join("sub/up")).unwrap();
        std::fs::hard_link(folder.join("run.sh"), folder.join("sub/run2.sh")).unwrap();
        let image = root.join("work.img");
        let packed = pack_with_room(&folder, &image, TEST_ROOM).unwrap();
        assert_eq!(
            (packed.files, packed.symbolic_links, packed.hard_links),
            (1, 3, 1)
        );
        assert_eq!((packed.folders, packed.skipped), (1, 0));
        let (mut volume, _) = Volume::mount(FileDisk::open(&image).unwrap()).unwrap();
        let top = eggos_eggfs::work_folder(&mut volume).unwrap();
        let link = volume.lookup(top, b"link").unwrap().unwrap();
        assert_eq!(eggos_eggfs::kind_of(link), Kind::SymbolicLink);
        assert_eq!(volume.link_target(link), Ok(b"run.sh".to_vec()));
        let script = volume.lookup(top, b"run.sh").unwrap().unwrap();
        assert_eq!(volume.attributes(script).unwrap().links, 2);
        volume.check().unwrap();
        drop(volume);
        let out = root.join("out");
        let unpacked = unpack(&image, &out).unwrap();
        assert_eq!((unpacked.files, unpacked.symbolic_links), (1, 3));
        assert_eq!((unpacked.hard_links, unpacked.skipped), (1, 0));
        let mode = std::fs::metadata(out.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o750);
        assert_eq!(
            std::fs::read_link(out.join("link")).unwrap(),
            Path::new("run.sh")
        );
        assert_eq!(
            std::fs::read_link(out.join("dangling")).unwrap(),
            Path::new("../outside/missing")
        );
        assert_eq!(
            std::fs::read_link(out.join("sub/up")).unwrap(),
            Path::new("..")
        );
        let first = std::fs::metadata(out.join("run.sh")).unwrap();
        let second = std::fs::metadata(out.join("sub/run2.sh")).unwrap();
        assert_eq!((first.ino(), first.nlink()), (second.ino(), 2));
        assert_eq!(
            std::fs::read(out.join("link")).unwrap(),
            b"#!/bin/eggsh\necho hi\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn files_written_on_the_disk_come_back_out_and_a_used_folder_is_refused() {
        let root = scratch("results");
        let folder = root.join("repo");
        write(&folder.join("src/lib.rs"), b"pub fn f() {}\n");
        let image = root.join("work.img");
        pack_with_room(&folder, &image, TEST_ROOM).unwrap();
        let (mut volume, _) = Volume::mount(FileDisk::open(&image).unwrap()).unwrap();
        let top = eggos_eggfs::work_folder(&mut volume).unwrap();
        let logs = volume
            .create(top, b"logs", Kind::Directory, 0o755, 5)
            .unwrap();
        let log = volume
            .create(logs, b"build.log", Kind::Regular, 0o644, 5)
            .unwrap();
        volume.write(log, None, b"Finished\n", 5).unwrap();
        let src = volume.lookup(top, b"src").unwrap().unwrap();
        volume.remove(src, b"lib.rs", false, 6).unwrap();
        volume.commit().unwrap();
        drop(volume);
        let out = root.join("out");
        write(&out.join("stale.txt"), b"x");
        assert!(
            unpack(&image, &out)
                .unwrap_err()
                .contains("not an empty folder")
        );
        std::fs::remove_file(out.join("stale.txt")).unwrap();
        unpack(&image, &out).unwrap();
        assert_eq!(
            tree(&out),
            [
                ("logs".to_owned(), None),
                ("logs/build.log".to_owned(), Some(b"Finished\n".to_vec())),
                ("src".to_owned(), None),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_image_that_is_not_a_work_disk_is_refused() {
        let root = scratch("not-work");
        let image = root.join("home.img");
        let file = File::create(&image).unwrap();
        file.set_len(4 << 20).unwrap();
        drop(file);
        let mut disk = FileDisk::open(&image).unwrap();
        eggos_eggfs::format(&mut disk, 1).unwrap();
        drop(disk);
        let error = unpack(&image, &root.join("out")).unwrap_err();
        assert!(error.contains("not a work disk"), "{error}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_image_has_room_beyond_the_folder_and_inodes_for_every_item() {
        assert_eq!(image_bytes(0, 0, HEADROOM_BYTES), HEADROOM_BYTES);
        let gib = 1u64 << 30;
        let large = image_bytes(8 * gib, 10, HEADROOM_BYTES);
        assert!(large >= 8 * gib + 8 * gib / 32 + HEADROOM_BYTES);
        assert_eq!(large % MEBIBYTE, 0);
        let many = image_bytes(0, 1_000_000, HEADROOM_BYTES);
        assert_eq!(image_bytes(0, 0, TEST_ROOM), TEST_ROOM);
        assert!(many / BLOCK_SIZE as u64 / 4 >= 2 * 1_000_000);
    }

    #[test]
    fn names_eggos_wrote_must_be_single_host_names() {
        let within = Path::new("out");
        assert_eq!(host_name(b"main.rs", within), Ok("main.rs".to_owned()));
        assert!(host_name(b"\xff", within).is_err());
        #[cfg(windows)]
        {
            assert!(host_name(b"a\\b", within).is_err());
            assert!(host_name(b"C:", within).is_err());
        }
    }

    #[test]
    fn results_go_beside_the_folder_by_default() {
        let out = default_output(Path::new("/tmp/repo")).unwrap();
        assert_eq!(out.file_name().unwrap(), "repo.out");
        assert_eq!(
            out.parent(),
            std::path::absolute(Path::new("/tmp/repo"))
                .unwrap()
                .parent()
        );
    }
}
