use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::vec::Vec;

use crate::hive::Hive;
use crate::path::{KeyPath, utf16};
use crate::registry::{HiveFile, Registry, Store};
use crate::store::{
    FileError, FileFacts, FileHandle, FileStore, HiveFiles, LOCK_SUFFIX, MACHINE_FOLDER_MODE,
    MACHINE_HIVE_MODE, USER_FOLDER_MODE, USER_HIVE_MODE, stamp_of,
};
use crate::value::{REG_SZ, string_data};
use crate::{Error, MAX_HIVE_BYTES};

const HOME: &[u8] = b"/home/ada";
const USER_HIVE: &[u8] = b"/home/ada/.local/share/eggos/windows/user.hive";
const MACHINE_HIVE: &[u8] = b"/var/lib/eggos/windows/machine.hive";

#[derive(Debug, Clone)]
struct FakeFile {
    bytes: Vec<u8>,
    mode: u32,
    inode: u64,
    modified: u64,
}

#[derive(Debug)]
struct Opened {
    path: Vec<u8>,
    position: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Created { locked: bool },
    Renamed { locked: bool },
}

#[derive(Debug, Default)]
struct Disk {
    files: BTreeMap<Vec<u8>, FakeFile>,
    folders: BTreeMap<Vec<u8>, u32>,
    opened: BTreeMap<FileHandle, Opened>,
    holders: BTreeMap<Vec<u8>, (u32, FileHandle)>,
    steps: Vec<Step>,
    next_handle: FileHandle,
    next_inode: u64,
    largest_read: Option<usize>,
    reported_size: Option<u64>,
    frozen_clock: bool,
    clock: u64,
}

impl Disk {
    fn lock_path(path: &[u8]) -> Vec<u8> {
        let hive = match path.iter().rposition(|byte| *byte == b'.') {
            Some(at) if path[..at].ends_with(b".hive.new") => &path[..at - 4],
            _ => path,
        };
        let mut lock = hive.to_vec();
        lock.extend_from_slice(LOCK_SUFFIX.as_bytes());
        lock
    }

    fn holds_lock(&self, pid: u32, path: &[u8]) -> bool {
        self.holders
            .get(&Disk::lock_path(path))
            .is_some_and(|(holder, _)| *holder == pid)
    }

    fn open(&mut self, path: &[u8]) -> FileHandle {
        self.next_handle += 1;
        self.opened.insert(
            self.next_handle,
            Opened {
                path: path.to_vec(),
                position: 0,
            },
        );
        self.next_handle
    }

    fn make(&mut self, path: &[u8], mode: u32) {
        self.next_inode += 1;
        let modified = self.clock;
        self.files.insert(
            path.to_vec(),
            FakeFile {
                bytes: Vec::new(),
                mode,
                inode: self.next_inode,
                modified,
            },
        );
    }

    fn file(&self, path: &[u8]) -> Option<&FakeFile> {
        self.files.get(path)
    }

    fn paths_holding(&self, part: &[u8]) -> Vec<Vec<u8>> {
        self.files
            .keys()
            .filter(|path| path.windows(part.len()).any(|window| window == part))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone)]
struct FakeFiles {
    disk: Rc<RefCell<Disk>>,
    pid: u32,
    user_id: u32,
}

impl HiveFiles for FakeFiles {
    fn open_to_read(&mut self, path: &[u8]) -> Result<FileHandle, FileError> {
        let mut disk = self.disk.borrow_mut();
        if disk.file(path).is_none() {
            return Err(FileError::NotFound);
        }
        Ok(disk.open(path))
    }

    fn create_new(&mut self, path: &[u8], mode: u32) -> Result<FileHandle, FileError> {
        let mut disk = self.disk.borrow_mut();
        if disk.file(path).is_some() {
            return Err(FileError::Exists);
        }
        let locked = disk.holds_lock(self.pid, path);
        disk.steps.push(Step::Created { locked });
        disk.make(path, mode);
        Ok(disk.open(path))
    }

    fn lock(&mut self, path: &[u8], mode: u32) -> Result<FileHandle, FileError> {
        let mut disk = self.disk.borrow_mut();
        if disk.holders.contains_key(path) {
            return Err(FileError::Failed);
        }
        if disk.file(path).is_none() {
            disk.make(path, mode);
        }
        let handle = disk.open(path);
        disk.holders.insert(path.to_vec(), (self.pid, handle));
        Ok(handle)
    }

    fn read(&mut self, file: FileHandle, buffer: &mut [u8]) -> Result<usize, FileError> {
        let mut disk = self.disk.borrow_mut();
        let largest = disk.largest_read.unwrap_or(usize::MAX);
        let opened = disk.opened.get(&file).ok_or(FileError::Failed)?;
        let path = opened.path.clone();
        let position = opened.position;
        let bytes = &disk.file(&path).ok_or(FileError::Failed)?.bytes;
        let count = bytes
            .len()
            .saturating_sub(position)
            .min(buffer.len())
            .min(largest);
        buffer[..count].copy_from_slice(&bytes[position..position + count]);
        disk.opened.get_mut(&file).unwrap().position += count;
        Ok(count)
    }

    fn write_all(&mut self, file: FileHandle, bytes: &[u8]) -> Result<(), FileError> {
        let mut disk = self.disk.borrow_mut();
        let path = disk
            .opened
            .get(&file)
            .ok_or(FileError::Failed)?
            .path
            .clone();
        disk.files
            .get_mut(&path)
            .ok_or(FileError::Failed)?
            .bytes
            .extend_from_slice(bytes);
        Ok(())
    }

    fn sync(&mut self, _: FileHandle) -> Result<(), FileError> {
        Ok(())
    }

    fn close(&mut self, file: FileHandle) {
        let mut disk = self.disk.borrow_mut();
        disk.opened.remove(&file);
        disk.holders.retain(|_, (_, handle)| *handle != file);
    }

    fn facts(&mut self, path: &[u8]) -> Option<FileFacts> {
        let disk = self.disk.borrow();
        let file = disk.file(path)?;
        Some(FileFacts {
            inode: file.inode,
            size: disk.reported_size.unwrap_or(file.bytes.len() as u64),
            modified_seconds: file.modified,
            modified_nanoseconds: 0,
        })
    }

    fn make_directory(&mut self, path: &[u8], mode: u32) -> Result<(), FileError> {
        let mut disk = self.disk.borrow_mut();
        if disk.folders.contains_key(path) {
            return Err(FileError::Exists);
        }
        disk.folders.insert(path.to_vec(), mode);
        Ok(())
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), FileError> {
        let mut disk = self.disk.borrow_mut();
        let mut file = disk.files.remove(from).ok_or(FileError::NotFound)?;
        let locked = disk.holds_lock(self.pid, from) || disk.holds_lock(self.pid, to);
        disk.steps.push(Step::Renamed { locked });
        file.modified = disk.clock;
        disk.files.insert(to.to_vec(), file);
        Ok(())
    }

    fn remove(&mut self, path: &[u8]) -> Result<(), FileError> {
        let mut disk = self.disk.borrow_mut();
        disk.files
            .remove(path)
            .map(|_| ())
            .ok_or(FileError::NotFound)
    }

    fn process_id(&mut self) -> u32 {
        self.pid
    }

    fn user_id(&mut self) -> u32 {
        self.user_id
    }

    fn now(&mut self) -> u64 {
        let mut disk = self.disk.borrow_mut();
        if !disk.frozen_clock {
            disk.clock += 1;
        }
        disk.clock
    }
}

fn process(disk: &Rc<RefCell<Disk>>, pid: u32, user_id: u32) -> FileStore<FakeFiles> {
    FileStore::new(
        FakeFiles {
            disk: Rc::clone(disk),
            pid,
            user_id,
        },
        HOME,
    )
}

fn key(text: &str) -> KeyPath {
    KeyPath::parse(text).unwrap()
}

fn set(registry: &mut Registry<FileStore<FakeFiles>>, key_text: &str, name: &str) {
    let path = key(key_text);
    registry.create_key(&path).unwrap();
    registry
        .set_value(&path, &utf16(name), REG_SZ, string_data(name))
        .unwrap();
}

fn with_suffix(path: &[u8], suffix: &str) -> Vec<u8> {
    let mut path = path.to_vec();
    path.extend_from_slice(suffix.as_bytes());
    path
}

#[test]
fn the_user_s_hive_is_its_owner_s_alone_and_the_machine_s_readable_by_all() {
    let disk = Rc::new(RefCell::new(Disk::default()));
    let mut registry = Registry::new(process(&disk, 10, 0));
    set(&mut registry, "HKCU\\Software\\AutoHotkey", "Version");
    set(&mut registry, "HKLM\\Software\\AutoHotkey", "InstallDir");
    let disk = disk.borrow();
    assert_eq!(disk.file(USER_HIVE).unwrap().mode, USER_HIVE_MODE);
    assert_eq!(USER_HIVE_MODE, 0o600);
    assert_eq!(disk.file(MACHINE_HIVE).unwrap().mode, MACHINE_HIVE_MODE);
    assert_eq!(MACHINE_HIVE_MODE, 0o644);
    assert_eq!(
        disk.file(&with_suffix(USER_HIVE, LOCK_SUFFIX))
            .unwrap()
            .mode,
        0o600
    );
    assert_eq!(
        disk.file(&with_suffix(MACHINE_HIVE, LOCK_SUFFIX))
            .unwrap()
            .mode,
        0o644
    );
    assert_eq!(
        disk.folders
            .get(&b"/home/ada/.local/share/eggos/windows"[..]),
        Some(&USER_FOLDER_MODE)
    );
    assert!(!disk.folders.contains_key(&b"/home"[..]));
    assert_eq!(
        disk.folders.get(&b"/var/lib/eggos/windows"[..]),
        Some(&MACHINE_FOLDER_MODE)
    );
    assert!(disk.paths_holding(b".new").is_empty());
}

#[test]
fn every_write_holds_the_hive_s_lock_and_lets_go_after() {
    let disk = Rc::new(RefCell::new(Disk::default()));
    let mut registry = Registry::new(process(&disk, 10, 1000));
    set(&mut registry, "HKCU\\Software\\AutoHotkey", "Version");
    registry.reset(HiveFile::User).unwrap();
    {
        let disk = disk.borrow();
        assert!(disk.steps.len() >= 6, "{:?}", disk.steps);
        assert!(
            disk.steps.iter().all(|step| matches!(
                step,
                Step::Created { locked: true } | Step::Renamed { locked: true }
            )),
            "{:?}",
            disk.steps
        );
        assert!(disk.holders.is_empty());
    }
    let mut other = process(&disk, 20, 1000);
    other.lock(HiveFile::User).unwrap();
    let path = key("HKCU\\Software\\AutoHotkey");
    let refused = registry.set_value(&path, &utf16("Count"), REG_SZ, string_data("2"));
    assert_eq!(refused, Err(Error::WriteFailed));
    other.unlock(HiveFile::User);
    assert!(disk.borrow().holders.is_empty());
    registry
        .set_value(&path, &utf16("Count"), REG_SZ, string_data("2"))
        .unwrap();
}

#[test]
fn two_processes_write_through_their_own_temporary_files_and_keep_both_changes() {
    let disk = Rc::new(RefCell::new(Disk::default()));
    let mut first = process(&disk, 10, 1000);
    let mut second = process(&disk, 20, 1000);
    let first_bytes = Hive::default().encode().unwrap();
    let mut second_bytes = first_bytes.clone();
    second_bytes.push(0);
    first.write_temporary(HiveFile::User, &first_bytes).unwrap();
    second
        .write_temporary(HiveFile::User, &second_bytes)
        .unwrap();
    first.replace_with_temporary(HiveFile::User).unwrap();
    assert_eq!(disk.borrow().file(USER_HIVE).unwrap().bytes, first_bytes);
    second.replace_with_temporary(HiveFile::User).unwrap();
    assert_eq!(disk.borrow().file(USER_HIVE).unwrap().bytes, second_bytes);
    assert_ne!(
        first.temporary_path(HiveFile::User),
        second.temporary_path(HiveFile::User)
    );
    disk.borrow_mut().files.remove(USER_HIVE);
    let mut ada = Registry::new(process(&disk, 10, 1000));
    let mut also_ada = Registry::new(process(&disk, 20, 1000));
    assert_eq!(ada.key_exists(&key("HKCU\\Software")), Ok(false));
    assert_eq!(also_ada.key_exists(&key("HKCU\\Software")), Ok(false));
    disk.borrow_mut().frozen_clock = true;
    set(&mut ada, "HKCU\\Software\\One", "First");
    set(&mut also_ada, "HKCU\\Software\\Two", "Second");
    let mut reader = Registry::new(process(&disk, 30, 1000));
    assert!(reader.key_exists(&key("HKCU\\Software\\One")).unwrap());
    assert!(reader.key_exists(&key("HKCU\\Software\\Two")).unwrap());
}

#[test]
fn a_leftover_temporary_file_of_this_process_is_replaced() {
    let disk = Rc::new(RefCell::new(Disk::default()));
    let mut store = process(&disk, 10, 1000);
    let leftover = store.temporary_path(HiveFile::User);
    disk.borrow_mut().make(&leftover, 0o666);
    disk.borrow_mut()
        .files
        .get_mut(&leftover)
        .unwrap()
        .bytes
        .extend_from_slice(b"half a hive");
    let bytes = Hive::default().encode().unwrap();
    store.write_temporary(HiveFile::User, &bytes).unwrap();
    store.replace_with_temporary(HiveFile::User).unwrap();
    let disk = disk.borrow();
    let hive = disk.file(USER_HIVE).unwrap();
    assert_eq!(hive.bytes, bytes);
    assert_eq!(hive.mode, USER_HIVE_MODE);
}

#[test]
fn a_read_goes_to_the_end_of_the_file_and_stops_past_the_cap() {
    let disk = Rc::new(RefCell::new(Disk::default()));
    let mut registry = Registry::new(process(&disk, 10, 1000));
    set(&mut registry, "HKCU\\Software\\AutoHotkey", "Version");
    let whole = disk.borrow().file(USER_HIVE).unwrap().bytes.clone();
    {
        let mut disk = disk.borrow_mut();
        disk.largest_read = Some(7);
        disk.reported_size = Some(3);
    }
    let mut store = process(&disk, 20, 1000);
    assert_eq!(store.read(HiveFile::User).unwrap().unwrap(), whole);
    {
        let mut disk = disk.borrow_mut();
        disk.largest_read = None;
        disk.files.get_mut(USER_HIVE).unwrap().bytes = vec![0x5a; MAX_HIVE_BYTES + 4096];
    }
    assert_eq!(
        store.read(HiveFile::User).unwrap().unwrap().len(),
        MAX_HIVE_BYTES + 1
    );
    let mut reader = Registry::new(store);
    assert_eq!(
        reader.hive(HiveFile::User).map(|_| ()),
        Err(Error::HiveDamaged)
    );
    let mut missing = process(&disk, 30, 1000);
    assert_eq!(missing.read(HiveFile::Machine), Ok(None));
}

#[test]
fn the_stamp_changes_when_a_rename_replaces_the_file_in_the_same_tick() {
    let facts = FileFacts {
        inode: 7,
        size: 24,
        modified_seconds: 1_700_000_000,
        modified_nanoseconds: 0,
    };
    assert_ne!(stamp_of(facts), stamp_of(FileFacts { inode: 8, ..facts }));
    let disk = Rc::new(RefCell::new(Disk {
        frozen_clock: true,
        ..Disk::default()
    }));
    let mut store = process(&disk, 10, 1000);
    let bytes = Hive::default().encode().unwrap();
    store.write_temporary(HiveFile::User, &bytes).unwrap();
    store.replace_with_temporary(HiveFile::User).unwrap();
    let before = store.stamp(HiveFile::User);
    store.write_temporary(HiveFile::User, &bytes).unwrap();
    store.replace_with_temporary(HiveFile::User).unwrap();
    assert_ne!(store.stamp(HiveFile::User), before);
}
