//! EggFS volumes in the VFS (D131): a disk formatted with EggOS's own journaled file system
//! (the `eggos-eggfs` crate), whose top-level directories are mounted as [`Folder`]s, `/home`
//! and `/Applications` at boot.
//!
//! - **One lock per volume**, a [`SleepLock`]: it is held across disk transfers, so a thread
//!   that wants the volume meanwhile sleeps instead of spinning, and the holder may wait for
//!   the disk as long as the disk takes.
//! - **Commits.** A change wakes the volume's committer, which waits [`COMMIT_DELAY`] and
//!   commits everything changed meanwhile; `sync`, `syncfs`, `fsync` and `fdatasync` commit
//!   at once ([`FileSystem::sync`]), and so does a change that would not fit the journal.
//!   Nothing runs while nothing changes.
//! - **Errors.** A disk failure or an inconsistency reads as `EIO`; a volume whose commit
//!   failed refuses every change after it.

use super::{
    Attributes, DirectoryEntry, FileSystem, Name, NodeKind, Refused, StorageError, WriteAt,
};
use crate::block::{BlockDevice, SECTOR_SIZE};
use crate::devices::Deadline;
use crate::sync::{SleepLock, Sleeping};
use crate::thread::WaitQueue;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};
use eggos_eggfs::{BLOCK_SIZE, Disk, DiskError, Error, Kind, MAX_NAME, Volume};

/// How long one disk request may take before it fails.
pub const REQUEST_TIMEOUT_MS: u64 = 2000;

/// How long the committer waits after a change before it commits, so the changes of a burst
/// of work share one transaction.
pub const COMMIT_DELAY: core::time::Duration = core::time::Duration::from_secs(5);

const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / SECTOR_SIZE) as u64;

const BLOCKS_PER_REQUEST: usize = 15;

/// A disk the kernel drives, addressed in EggFS blocks: each transfer of up to 60 KiB gets
/// [`REQUEST_TIMEOUT_MS`].
pub struct BlockDisk {
    device: Box<dyn BlockDevice>,
    new_deadline: fn(u64) -> Deadline,
}

impl core::fmt::Debug for BlockDisk {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BlockDisk")
            .field("sectors", &self.device.sectors())
            .finish_non_exhaustive()
    }
}

impl BlockDisk {
    /// `device`, each request polling until a deadline `new_deadline` makes.
    pub fn new(device: Box<dyn BlockDevice>, new_deadline: fn(u64) -> Deadline) -> Self {
        Self {
            device,
            new_deadline,
        }
    }

    /// Whether the disk refuses writes.
    pub fn is_read_only(&self) -> bool {
        self.device.is_read_only()
    }

    /// Gives the device back.
    pub fn into_device(self) -> Box<dyn BlockDevice> {
        self.device
    }

    fn deadline(&self) -> Deadline {
        (self.new_deadline)(REQUEST_TIMEOUT_MS)
    }
}

impl Disk for BlockDisk {
    fn blocks(&self) -> u64 {
        self.device.sectors() / SECTORS_PER_BLOCK
    }

    fn read(&mut self, block: u64, buffer: &mut [u8]) -> Result<(), DiskError> {
        for (index, chunk) in buffer
            .chunks_mut(BLOCKS_PER_REQUEST * BLOCK_SIZE)
            .enumerate()
        {
            let sector = (block + (index * BLOCKS_PER_REQUEST) as u64) * SECTORS_PER_BLOCK;
            let mut deadline = self.deadline();
            self.device
                .read(sector, chunk, &mut deadline)
                .map_err(|_| DiskError)?;
        }
        Ok(())
    }

    fn write(&mut self, block: u64, buffer: &[u8]) -> Result<(), DiskError> {
        for (index, chunk) in buffer.chunks(BLOCKS_PER_REQUEST * BLOCK_SIZE).enumerate() {
            let sector = (block + (index * BLOCKS_PER_REQUEST) as u64) * SECTORS_PER_BLOCK;
            let mut deadline = self.deadline();
            self.device
                .write(sector, chunk, &mut deadline)
                .map_err(|_| DiskError)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        let mut deadline = self.deadline();
        self.device.flush(&mut deadline).map_err(|_| DiskError)
    }
}

/// A mounted EggFS volume on disk `D`: see the module documentation.
pub struct EggFs<D: Disk + Send> {
    volume: SleepLock<Volume<D>>,
    device: (u32, u32),
    sleeping: Sleeping,
    changed: AtomicBool,
    committer: WaitQueue,
}

impl<D: Disk + Send> core::fmt::Debug for EggFs<D> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EggFs")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

fn refused(error: Error) -> Refused {
    match error {
        Error::NotFound => Refused::NotFound,
        Error::Exists => Refused::Exists,
        Error::NotDirectory => Refused::NotDirectory,
        Error::IsDirectory => Refused::IsDirectory,
        Error::NotEmpty => Refused::NotEmpty,
        Error::NoSpace => Refused::NoSpace,
        Error::NameTooLong => Refused::NameTooLong,
        Error::InvalidArgument => Refused::InvalidArgument,
        Error::TooBig => Refused::TooBig,
        Error::Busy => Refused::Busy,
        Error::Corrupt | Error::Io => Refused::Storage,
    }
}

fn node_kind(node: u64) -> NodeKind {
    match eggos_eggfs::kind_of(node) {
        Kind::Directory => NodeKind::Directory,
        Kind::Regular => NodeKind::Regular,
    }
}

impl<D: Disk + Send> EggFs<D> {
    /// `volume`, reporting device number `device`, its lock's waiters waiting as `sleeping`
    /// says.
    pub fn new(volume: Volume<D>, device: (u32, u32), sleeping: Sleeping) -> Self {
        Self {
            volume: SleepLock::new(volume, sleeping),
            device,
            sleeping,
            changed: AtomicBool::new(false),
            committer: WaitQueue::new(),
        }
    }

    /// Runs `with` on the volume, holding its lock.
    pub fn with_volume<R>(&self, with: impl FnOnce(&mut Volume<D>) -> R) -> R {
        with(&mut self.volume.lock())
    }

    /// The top-level directory `name` as a [`Folder`] to mount, made (permissions 0755, time
    /// `now`) and committed if the volume lacks it.
    pub fn folder(&'static self, name: &[u8], now: u64) -> Result<Folder<D>, Error> {
        let mut volume = self.volume.lock();
        let root = volume.root();
        let node = match volume.lookup(root, name)? {
            Some(node) if eggos_eggfs::kind_of(node) == Kind::Directory => node,
            Some(_) => return Err(Error::NotDirectory),
            None => {
                let node = volume.create(root, name, Kind::Directory, 0o755, now)?;
                volume.commit()?;
                node
            }
        };
        Ok(Folder {
            eggfs: self,
            root: node,
        })
    }

    /// Commits every change made so far.
    pub fn commit(&self) -> Result<(), StorageError> {
        self.volume.lock().commit().map_err(|_| StorageError)
    }

    fn changed(&self) {
        if !self.changed.swap(true, Ordering::AcqRel) {
            self.sleeping.wake(&self.committer);
        }
    }

    /// The committer's loop: waits for a change, has `pause` wait [`COMMIT_DELAY`], commits,
    /// and waits again. Never returns.
    pub fn commit_changes(&self, pause: &dyn Fn()) -> ! {
        loop {
            self.sleeping.wait(&self.committer, &mut || {
                self.changed.load(Ordering::Acquire)
            });
            pause();
            self.changed.store(false, Ordering::Release);
            let _ = self.commit();
        }
    }
}

/// A directory of an [`EggFs`] volume mounted as a file system of its own: lookups, changes
/// and renames stay inside it, and `..` from its root leaves through its mount point.
pub struct Folder<D: Disk + Send + 'static> {
    eggfs: &'static EggFs<D>,
    root: u64,
}

impl<D: Disk + Send + 'static> core::fmt::Debug for Folder<D> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Folder")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl<D: Disk + Send + 'static> Folder<D> {
    fn volume(&self) -> crate::sync::SleepLockGuard<'_, Volume<D>> {
        self.eggfs.volume.lock()
    }

    fn changing<R>(
        &self,
        change: impl FnOnce(&mut Volume<D>) -> Result<R, Error>,
    ) -> Result<R, Refused> {
        let result = change(&mut self.volume());
        self.eggfs.changed();
        result.map_err(refused)
    }
}

impl<D: Disk + Send + 'static> FileSystem for Folder<D> {
    fn root(&self) -> u64 {
        self.root
    }

    fn device(&self) -> (u32, u32) {
        self.eggfs.device
    }

    fn kind(&self, node: u64) -> NodeKind {
        node_kind(node)
    }

    fn attributes(&self, node: u64) -> Attributes {
        let kind = node_kind(node);
        match self.volume().attributes(node) {
            Ok(found) => Attributes {
                kind,
                permissions: found.permissions,
                file_id: u64::from(found.inode),
                links: found.links,
                owner: found.owner,
                group: found.group,
                size: found.size,
                modified: found.modified,
            },
            Err(_) => Attributes {
                kind,
                permissions: 0,
                file_id: u64::from(node as u32),
                links: 0,
                owner: 0,
                group: 0,
                size: 0,
                modified: 0,
            },
        }
    }

    fn lookup(&self, directory: u64, name: &[u8]) -> Option<u64> {
        self.volume().lookup(directory, name).ok().flatten()
    }

    fn parent(&self, node: u64) -> u64 {
        if node == self.root {
            return self.root;
        }
        self.volume().parent(node).unwrap_or(self.root)
    }

    fn name(&self, node: u64) -> Name {
        if node == self.root {
            return Name::new(b"");
        }
        let mut name = [0u8; MAX_NAME];
        match self.volume().name(node, &mut name) {
            Ok(length) => Name::new(&name[..length]),
            Err(_) => Name::new(b""),
        }
    }

    fn read(&self, _node: u64, _offset: u64, _length: usize) -> &[u8] {
        &[]
    }

    fn copies_contents(&self) -> bool {
        true
    }

    fn read_into(&self, node: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, StorageError> {
        match self.volume().read(node, offset, buffer) {
            Ok(read) => Ok(read),
            Err(Error::NotFound | Error::IsDirectory) => Ok(0),
            Err(_) => Err(StorageError),
        }
    }

    fn is_writable(&self) -> bool {
        true
    }

    fn write(
        &self,
        node: u64,
        at: WriteAt,
        bytes: &[u8],
        now: u64,
    ) -> Result<(usize, u64), Refused> {
        let offset = match at {
            WriteAt::Offset(offset) => Some(offset),
            WriteAt::End => None,
        };
        self.changing(|volume| volume.write(node, offset, bytes, now))
    }

    fn truncate(&self, node: u64, length: u64, now: u64) -> Result<(), Refused> {
        self.changing(|volume| volume.truncate(node, length, now))
    }

    fn create(
        &self,
        directory: u64,
        name: &[u8],
        kind: NodeKind,
        permissions: u32,
        now: u64,
    ) -> Result<u64, Refused> {
        let kind = match kind {
            NodeKind::Directory => Kind::Directory,
            NodeKind::Regular => Kind::Regular,
            NodeKind::CharacterDevice => return Err(Refused::InvalidArgument),
        };
        self.changing(|volume| volume.create(directory, name, kind, permissions, now))
    }

    fn remove(
        &self,
        directory: u64,
        name: &[u8],
        directories: bool,
        now: u64,
    ) -> Result<(), Refused> {
        self.changing(|volume| volume.remove(directory, name, directories, now))
    }

    fn rename(
        &self,
        from: u64,
        from_name: &[u8],
        to: u64,
        to_name: &[u8],
        no_replace: bool,
        now: u64,
    ) -> Result<(), Refused> {
        self.changing(|volume| volume.rename(from, from_name, to, to_name, no_replace, now))
    }

    fn set_modified(&self, node: u64, time: u64) -> Result<(), Refused> {
        self.changing(|volume| volume.set_modified(node, time))
    }

    fn entry(&self, directory: u64, position: u64) -> Option<DirectoryEntry> {
        let entry = self.volume().entry(directory, position).ok()??;
        Some(DirectoryEntry {
            name: Name::new(entry.name()),
            node: entry.node,
            kind: node_kind(entry.node),
        })
    }

    fn sync(&self) -> Result<(), StorageError> {
        self.eggfs.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{Node, resolve};
    use eggos_eggfs::memory::MemoryDisk;

    fn mounted() -> (&'static EggFs<MemoryDisk>, Node, Node) {
        let mut disk = MemoryDisk::new(2048);
        eggos_eggfs::format(&mut disk, 0).unwrap();
        let volume = Volume::mount(disk).unwrap().0;
        let eggfs: &'static EggFs<MemoryDisk> =
            Box::leak(Box::new(EggFs::new(volume, (254, 1), Sleeping::spinning())));
        let home: &'static Folder<MemoryDisk> =
            Box::leak(Box::new(eggfs.folder(b"home", 1).unwrap()));
        let applications: &'static Folder<MemoryDisk> =
            Box::leak(Box::new(eggfs.folder(b"Applications", 1).unwrap()));
        (eggfs, Node::root_of(home), Node::root_of(applications))
    }

    fn read_all(node: Node) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec::Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = node
                .file_system()
                .read_into(node.id(), out.len() as u64, &mut buffer)
                .unwrap();
            if read == 0 {
                return out;
            }
            out.extend_from_slice(&buffer[..read]);
        }
    }

    #[test]
    fn folders_change_and_read_back_through_the_vfs_and_survive_a_commit() {
        let (eggfs, home, applications) = mounted();
        let fs = home.file_system();
        assert!(fs.is_writable());
        let docs = fs
            .create(home.id(), b"docs", NodeKind::Directory, 0o755, 2)
            .unwrap();
        let file = fs
            .create(docs, b"a.txt", NodeKind::Regular, 0o644, 3)
            .unwrap();
        assert_eq!(fs.write(file, WriteAt::End, b"hello\n", 4), Ok((6, 6)));
        assert_eq!(fs.write(file, WriteAt::End, b"again\n", 5), Ok((6, 12)));
        let node = resolve(home, home, b"docs/a.txt").unwrap();
        assert_eq!(read_all(node), b"hello\nagain\n");
        assert_eq!(node.attributes().size, 12);
        assert_eq!(node.parent().parent(), home);
        assert_eq!(fs.name(file).as_bytes(), b"a.txt");
        assert_eq!(
            fs.create(file, b"x", NodeKind::Regular, 0, 0),
            Err(Refused::NotDirectory)
        );
        assert_eq!(
            fs.create(docs, b"tty", NodeKind::CharacterDevice, 0, 0),
            Err(Refused::InvalidArgument)
        );
        assert_eq!(fs.entry(home.id(), 0).unwrap().name.as_bytes(), b"docs");
        assert_eq!(fs.entry(home.id(), 1), None);
        assert_eq!(applications.file_system().entry(applications.id(), 0), None);
        fs.sync().unwrap();
        assert!(!eggfs.with_volume(|volume| volume.has_changes()));
        eggfs.with_volume(|volume| volume.check().unwrap());
        fs.rename(docs, b"a.txt", home.id(), b"b.txt", false, 6)
            .unwrap();
        fs.remove(home.id(), b"docs", true, 7).unwrap();
        fs.truncate(file, 3, 8).unwrap();
        assert_eq!(read_all(resolve(home, home, b"b.txt").unwrap()), b"hel");
        assert_eq!(
            fs.remove(home.id(), b"missing", false, 9),
            Err(Refused::NotFound)
        );
        eggfs.with_volume(|volume| volume.check().unwrap());
    }
}
