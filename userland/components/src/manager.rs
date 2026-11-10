//! Finding the store, reading what is installed, and installing and removing components
//! (D235). A component is installed under [`COMPONENTS`]`/<name>/`: its files below `root/`,
//! at the paths the base links to, and its record `component` beside them. An install copies
//! into a staging folder, checks every file's SHA-256 as it is copied, writes the record, and
//! only then renames the staging folder into place, so a failure at any step leaves that
//! component uninstalled; a remove renames the folder away first, then deletes it. Nothing here follows a
//! symbolic link inside the components folder, and the folders above it must be folders.

use crate::index::{self, Component, Header, Index, IndexError, SECTOR};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use eggos_app::files::{Files, Handle, Kind};
use eggos_crypto::sha2::Sha256;
use eggos_runtime::errno::Errno;

/// Where installed components live.
pub const COMPONENTS: &str = "/var/lib/eggos/components";
/// The folders above [`COMPONENTS`], made when missing, each a real folder.
pub const PARENTS: [&str; 3] = ["/var/lib", "/var/lib/eggos", COMPONENTS];
/// An installed component's record, in its folder.
pub const RECORD: &str = "component";
/// The folder in a component's folder that holds its files.
pub const ROOT: &str = "root";
/// The name prefix of an install in progress.
pub const STAGING_PREFIX: &str = ".staging-";
/// The name prefix of a remove in progress.
pub const REMOVING_PREFIX: &str = ".removing-";
/// The disks a store may be attached as.
pub const DISKS: [&str; 4] = ["/dev/vda", "/dev/vdb", "/dev/vdc", "/dev/vdd"];
/// The host share, where a store may also be.
pub const HOST_SHARE: &str = "/media/host";
/// The lock the one action at a time takes: a symbolic link whose target is
/// `<pid> install <title>` or `<pid> remove <title>`, made in one step, so no one ever reads it
/// half written.
pub const LOCK: &str = "/run/eggos/components.lock";
/// What follows [`LOCK`] in the name of a claim to take over a lock whose process is gone: a
/// symbolic link whose target is the claiming process's ID.
pub const TAKE_OVER_SUFFIX: &str = ".take-over-";
/// The most bytes of a process's command line read to see what it runs.
pub const MAX_COMMAND_LINE: usize = 4096;
/// The version of EggOS this manager belongs to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const COPY_CHUNK: usize = 64 * 1024;
const LOCK_TRIES: usize = 3;
const MAX_DEPTH: usize = 32;
const NAME_BYTES: usize = 256;
const PROGRAM_SUFFIXES: [&[u8]; 3] = [b".exe", b".EXE", b".app"];

/// Where the store is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An attached disk, `/dev/vdc`.
    Disk(&'static str),
    /// A file in the host share.
    Share,
}

/// The attached store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    /// Where it is.
    pub source: Source,
    /// The path it is read from.
    pub path: String,
    /// Its index.
    pub index: Index,
}

impl Store {
    /// The store's file name, `eggos-0.3.0-x86_64-components.img`.
    pub fn file_name(&self) -> String {
        store_file_name(&self.index.version, &self.index.arch)
    }
}

/// The file name of the store of `version` for `arch`.
pub fn store_file_name(version: &str, arch: &str) -> String {
    let mut name = String::from("eggos-");
    name.push_str(version);
    name.push('-');
    name.push_str(arch);
    name.push_str("-components.img");
    name
}

/// Why an install or a remove did not happen, said before anything changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No store is attached.
    NoStore,
    /// Neither the store nor the installed records know the name.
    NoSuchComponent(String),
    /// Another install or remove is running.
    Busy { title: String, removing: bool },
    /// Installed components need it: their titles.
    NeededBy(Vec<String>),
    /// Programs that need it are running: up to three names, and how many in all.
    InUse { names: Vec<String>, count: usize },
    /// Only root installs and removes components.
    NotAllowed,
}

/// Why an install or a remove failed part way: the component was not installed, or stays
/// installed, but after [`Failure::FilesLeft`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// A file's bytes did not match the store's SHA-256: the component's file name.
    Damaged(String),
    /// The store could not be read before the copy finished.
    Detached,
    /// The disk is full: bytes needed and bytes free.
    NoSpace { needed: u64, free: u64 },
    /// The person cancelled the copy.
    Cancelled,
    /// Only root installs and removes components.
    NotAllowed,
    /// A folder on the way is not a real folder.
    UnsafeFolder,
    /// The component is removed, but some of its files could not be deleted; the next install
    /// or remove deletes them.
    FilesLeft(Errno),
    /// Anything else.
    Errno(Errno),
}

/// Why [`remove_checked`] removed nothing, or not all of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError {
    /// Refused before anything changed.
    Refused(Refusal),
    /// Failed part way.
    Failed(Failure),
}

/// Reads the store at `path`, if it is one for `arch`.
pub fn read_store<F: Files>(
    files: &mut F,
    path: &str,
    source: Source,
    arch: &str,
) -> Result<Store, IndexError> {
    let length = files
        .status_following(path.as_bytes())
        .map_err(|_| IndexError::NotAStore)?
        .size;
    let handle = files
        .open(path.as_bytes())
        .map_err(|_| IndexError::NotAStore)?;
    let read = read_index(files, handle, length);
    files.close(handle);
    let index = read?;
    if index.arch != arch {
        return Err(IndexError::NotAStore);
    }
    Ok(Store {
        source,
        path: path.into(),
        index,
    })
}

fn read_index<F: Files>(files: &mut F, handle: Handle, length: u64) -> Result<Index, IndexError> {
    let mut first = [0u8; SECTOR as usize];
    read_exactly(files, handle, 0, &mut first).map_err(|_| IndexError::NotAStore)?;
    let header = Header::parse(&first)?;
    let mut text = vec![0u8; header.index_bytes as usize];
    read_exactly(files, handle, SECTOR, &mut text).map_err(|_| IndexError::IndexSize)?;
    index::parse_index(&header, &text, length)
}

fn read_exactly<F: Files>(
    files: &mut F,
    handle: Handle,
    mut offset: u64,
    mut buffer: &mut [u8],
) -> Result<(), Errno> {
    while !buffer.is_empty() {
        match files.read_at(handle, offset, buffer)? {
            0 => return Err(Errno::EIO),
            read => {
                offset += read as u64;
                buffer = &mut buffer[read..];
            }
        }
    }
    Ok(())
}

/// Finds the store for `arch`: an attached disk first, then the host share.
pub fn find_store<F: Files>(files: &mut F, arch: &str) -> Option<Store> {
    for disk in DISKS {
        if let Ok(store) = read_store(files, disk, Source::Disk(disk), arch) {
            return Some(store);
        }
    }
    let mut path = String::from(HOST_SHARE);
    path.push('/');
    path.push_str(&store_file_name(VERSION, arch));
    read_store(files, &path, Source::Share, arch).ok()
}

/// The installed components, in the order their folders are listed; a folder without a valid
/// record, or whose record names another component, is not one.
pub fn installed<F: Files>(files: &mut F) -> Vec<Component> {
    let mut found = Vec::new();
    let mut name = [0u8; NAME_BYTES];
    let mut at = 0;
    while let Ok(Some(length)) = files.entry(COMPONENTS.as_bytes(), at, &mut name) {
        at += 1;
        let Ok(name) = core::str::from_utf8(&name[..length]) else {
            continue;
        };
        if !index::is_name(name) {
            continue;
        }
        if let Some(component) = read_record(files, name) {
            found.push(component);
        }
    }
    found
}

fn read_record<F: Files>(files: &mut F, name: &str) -> Option<Component> {
    let folder = component_folder(name);
    if files.status(folder.as_bytes()).ok()?.kind != Kind::Directory {
        return None;
    }
    let path = join(&folder, RECORD);
    let status = files.status(path.as_bytes()).ok()?;
    if status.kind != Kind::File || status.size > u64::from(index::MAX_INDEX_BYTES) {
        return None;
    }
    let handle = files.open(path.as_bytes()).ok()?;
    let mut text = vec![0u8; status.size as usize];
    let read = read_exactly(files, handle, 0, &mut text);
    files.close(handle);
    read.ok()?;
    index::parse_record(&text)
        .ok()
        .filter(|component| component.name == name)
}

/// The folder component `name` is installed in.
pub fn component_folder(name: &str) -> String {
    join(COMPONENTS, name)
}

/// Where the installed file `path` of component `name` is.
pub fn installed_path(name: &str, path: &str) -> String {
    join(&join(&component_folder(name), ROOT), path)
}

fn join(folder: &str, name: &str) -> String {
    let mut path = String::from(folder);
    path.push('/');
    path.push_str(name);
    path
}

/// What installing `name` installs: the components it needs that are not installed, in the
/// index's order, then itself; empty when it is installed already.
pub fn plan_install<'a>(
    index: &'a Index,
    installed: &[Component],
    name: &str,
) -> Result<Vec<&'a Component>, Refusal> {
    let wanted = index
        .component(name)
        .ok_or_else(|| Refusal::NoSuchComponent(name.into()))?;
    let is_installed = |name: &str| installed.iter().any(|component| component.name == name);
    if is_installed(name) {
        return Ok(Vec::new());
    }
    let mut needed: Vec<&str> = vec![name];
    for component in index.components.iter().rev() {
        if needed.contains(&component.name.as_str()) {
            for need in &component.needs {
                if !needed.contains(&need.as_str()) && !is_installed(need) {
                    needed.push(need);
                }
            }
        }
    }
    let plan: Vec<&Component> = index
        .components
        .iter()
        .filter(|component| needed.contains(&component.name.as_str()))
        .collect();
    match plan.last() {
        Some(last) if last.name == wanted.name => Ok(plan),
        _ => Err(Refusal::NoSuchComponent(name.into())),
    }
}

/// Checks that `name` may be removed: no installed component needs it and no running program
/// runs one of its files. `Ok(None)` when it is not installed.
pub fn check_remove<F: Files>(
    files: &mut F,
    installed: &[Component],
    name: &str,
) -> Result<Option<Component>, Refusal> {
    let Some(component) = installed.iter().find(|component| component.name == name) else {
        return Ok(None);
    };
    let needed_by: Vec<String> = installed
        .iter()
        .filter(|other| other.needs(name))
        .map(|other| other.title.clone())
        .collect();
    if !needed_by.is_empty() {
        return Err(Refusal::NeededBy(needed_by));
    }
    let (names, count) = in_use(files, component);
    if count > 0 {
        return Err(Refusal::InUse { names, count });
    }
    Ok(Some(component.clone()))
}

/// The running programs that run one of `component`'s files: up to three names, and how many.
/// A program runs a file when its `argv[0]`, the link `argv[0]` names (`/bin/windows`), or its
/// `/proc/<pid>/exe` and the link that names is the file at its fixed or installed path.
pub fn in_use<F: Files>(files: &mut F, component: &Component) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let mut count = 0;
    let mut name = [0u8; NAME_BYTES];
    let mut line = vec![0u8; MAX_COMMAND_LINE];
    let mut target = vec![0u8; MAX_COMMAND_LINE];
    let mut at = 0;
    while let Ok(Some(length)) = files.entry(b"/proc", at, &mut name) {
        at += 1;
        let pid = &name[..length];
        if pid.is_empty() || !pid.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let mut path = String::from("/proc/");
        path.push_str(core::str::from_utf8(pid).unwrap_or("0"));
        path.push_str("/cmdline");
        let Ok(handle) = files.open(path.as_bytes()) else {
            continue;
        };
        let read = files.read(handle, &mut line).unwrap_or(0);
        files.close(handle);
        let mut words = line[..read].split(|&byte| byte == 0);
        let program = words.next().unwrap_or(b"");
        let pid = core::str::from_utf8(pid).unwrap_or("0");
        if !runs_component(files, pid, program, component, &mut target) {
            continue;
        }
        count += 1;
        if names.len() < 3 {
            let shown = match words.next().filter(|word| names_a_program(word)) {
                Some(word) => program_name(word),
                None => program_name(program),
            };
            names.push(shown);
        }
    }
    (names, count)
}

fn names_component_file(path: &[u8], component: &Component) -> bool {
    component.files.iter().any(|file| {
        let installed = installed_path(&component.name, &file.path);
        path == installed.as_bytes()
            || (path.first() == Some(&b'/') && &path[1..] == file.path.as_bytes())
    })
}

fn runs_component<F: Files>(
    files: &mut F,
    pid: &str,
    program: &[u8],
    component: &Component,
    target: &mut [u8],
) -> bool {
    if names_component_file(program, component) {
        return true;
    }
    if program.first() == Some(&b'/')
        && let Ok(length) = files.read_link(program, target)
        && names_component_file(&target[..length], component)
    {
        return true;
    }
    let exe = ["/proc/", pid, "/exe"].concat();
    let Ok(length) = files.read_link(exe.as_bytes(), target) else {
        return false;
    };
    if names_component_file(&target[..length], component) {
        return true;
    }
    let executable = target[..length].to_vec();
    files
        .read_link(&executable, target)
        .is_ok_and(|length| names_component_file(&target[..length], component))
}

/// Whether this process runs one of `component`'s files: `/proc/self/exe`, or the link that
/// names, is the file at its fixed or installed path.
pub fn runs_this_program<F: Files>(files: &mut F, component: &Component) -> bool {
    let mut target = vec![0u8; MAX_COMMAND_LINE];
    let Ok(length) = files.read_link(b"/proc/self/exe", &mut target) else {
        return false;
    };
    if names_component_file(&target[..length], component) {
        return true;
    }
    let executable = target[..length].to_vec();
    files
        .read_link(&executable, &mut target)
        .is_ok_and(|length| names_component_file(&target[..length], component))
}

fn names_a_program(word: &[u8]) -> bool {
    word.contains(&b'/') || PROGRAM_SUFFIXES.iter().any(|suffix| word.ends_with(suffix))
}

fn program_name(path: &[u8]) -> String {
    let name = eggos_app::files::file_name(path);
    let name = PROGRAM_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(*suffix))
        .unwrap_or(name);
    String::from_utf8_lossy(name).into_owned()
}

/// Takes the one-action-at-a-time lock for installing or removing `title`, after removing a
/// lock whose process is gone; who holds it otherwise.
pub fn lock<F: Files>(files: &mut F, title: &str, removing: bool, pid: u32) -> Result<(), Refusal> {
    let mut text = pid.to_string();
    text.push_str(if removing { " remove " } else { " install " });
    text.push_str(title);
    for _ in 0..LOCK_TRIES {
        match files.make_link(text.as_bytes(), LOCK.as_bytes()) {
            Ok(()) => return Ok(()),
            Err(Errno::EEXIST) => {
                let (holder, held_title, held_removing) = read_lock(files);
                if is_running(files, holder) || !take_over(files, holder, pid) {
                    return Err(Refusal::Busy {
                        title: held_title,
                        removing: held_removing,
                    });
                }
            }
            Err(Errno::EACCES | Errno::EPERM) => return Err(Refusal::NotAllowed),
            Err(_) => {
                let _ = files.make_directory(b"/run/eggos");
            }
        }
    }
    Err(Refusal::Busy {
        title: title.into(),
        removing,
    })
}

fn is_running<F: Files>(files: &mut F, pid: u32) -> bool {
    let mut proc = String::from("/proc/");
    proc.push_str(&pid.to_string());
    pid != 0 && files.status(proc.as_bytes()).is_ok()
}

/// The claim a process makes, as a symbolic link to its own ID, before deleting the lock of
/// `holder`, a process that is gone: only one process at a time may take that lock over, and
/// a claim whose process is gone too is deleted first.
pub fn take_over_claim(holder: u32) -> String {
    let mut claim = String::from(LOCK);
    claim.push_str(TAKE_OVER_SUFFIX);
    claim.push_str(&holder.to_string());
    claim
}

fn take_over<F: Files>(files: &mut F, holder: u32, pid: u32) -> bool {
    let claim = take_over_claim(holder);
    if !make_claim(files, &claim, pid) {
        return false;
    }
    if read_lock(files).0 == holder && !is_running(files, holder) {
        let _ = files.remove_file(LOCK.as_bytes());
    }
    let _ = files.remove_file(claim.as_bytes());
    true
}

fn make_claim<F: Files>(files: &mut F, claim: &str, pid: u32) -> bool {
    let claimer = pid.to_string();
    match files.make_link(claimer.as_bytes(), claim.as_bytes()) {
        Ok(()) => true,
        Err(Errno::EEXIST) => {
            let mut text = [0u8; 16];
            let stale = files
                .read_link(claim.as_bytes(), &mut text)
                .ok()
                .and_then(|length| core::str::from_utf8(text.get(..length)?).ok())
                .and_then(|text| text.parse().ok())
                .is_none_or(|other| !is_running(files, other));
            stale
                && files.remove_file(claim.as_bytes()).is_ok()
                && files
                    .make_link(claimer.as_bytes(), claim.as_bytes())
                    .is_ok()
        }
        Err(_) => false,
    }
}

/// Who holds the lock: their process ID, the title, and whether they remove it; `None` when
/// no one does or the lock cannot be read. The process may be gone.
pub fn lock_holder<F: Files>(files: &mut F) -> Option<(u32, String, bool)> {
    let mut text = vec![0u8; MAX_COMMAND_LINE];
    let length = files.read_link(LOCK.as_bytes(), &mut text).ok()?;
    let text = core::str::from_utf8(text.get(..length)?).ok()?;
    let mut words = text.splitn(3, ' ');
    let pid = words.next()?.parse().ok()?;
    let removing = words.next()? == "remove";
    Some((pid, words.next()?.into(), removing))
}

fn read_lock<F: Files>(files: &mut F) -> (u32, String, bool) {
    lock_holder(files).unwrap_or((0, String::new(), false))
}

/// Gives the lock back.
pub fn unlock<F: Files>(files: &mut F) {
    let _ = files.remove_file(LOCK.as_bytes());
}

/// Finishes what an interrupted install or remove left: removes every staging and removing
/// folder.
pub fn tidy<F: Files>(files: &mut F) {
    let mut name = [0u8; NAME_BYTES];
    let mut at = 0;
    while let Ok(Some(length)) = files.entry(COMPONENTS.as_bytes(), at, &mut name) {
        let entry = &name[..length];
        if entry.starts_with(STAGING_PREFIX.as_bytes())
            || entry.starts_with(REMOVING_PREFIX.as_bytes())
        {
            let path = join(COMPONENTS, &String::from_utf8_lossy(entry));
            if remove_tree(files, &path, 0).is_ok() {
                continue;
            }
        }
        at += 1;
    }
}

fn prepare_folders<F: Files>(files: &mut F) -> Result<(), Failure> {
    for folder in PARENTS {
        match files.make_directory(folder.as_bytes()) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(errno) => return Err(failure(errno)),
        }
        match files.status(folder.as_bytes()) {
            Ok(status) if status.kind == Kind::Directory => {}
            Ok(_) => return Err(Failure::UnsafeFolder),
            Err(errno) => return Err(failure(errno)),
        }
    }
    Ok(())
}

fn failure(errno: Errno) -> Failure {
    match errno {
        Errno::EACCES | Errno::EPERM => Failure::NotAllowed,
        errno => Failure::Errno(errno),
    }
}

/// Installs `component` from `store`, calling `progress` with the bytes copied and the total
/// after each piece; `progress` returning false cancels. Nothing is installed unless all of it
/// is.
pub fn install<F: Files>(
    files: &mut F,
    store: &Store,
    component: &Component,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<(), Failure> {
    prepare_folders(files)?;
    tidy(files);
    let total = component.size_bytes();
    if let Ok(space) = files.space(COMPONENTS.as_bytes())
        && space.available < total
    {
        return Err(Failure::NoSpace {
            needed: total,
            free: space.available,
        });
    }
    let staging = join(
        COMPONENTS,
        &[STAGING_PREFIX, component.name.as_str()].concat(),
    );
    files.make_directory(staging.as_bytes()).map_err(failure)?;
    let copied = copy_component(files, store, component, &staging, total, progress);
    let committed = copied.and_then(|()| {
        let target = component_folder(&component.name);
        files.sync();
        files
            .rename(staging.as_bytes(), target.as_bytes())
            .map_err(failure)?;
        files.sync();
        Ok(())
    });
    if committed.is_err() {
        let _ = remove_tree(files, &staging, 0);
        files.sync();
    }
    committed
}

fn copy_component<F: Files>(
    files: &mut F,
    store: &Store,
    component: &Component,
    staging: &str,
    total: u64,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<(), Failure> {
    let source = files
        .open(store.path.as_bytes())
        .map_err(|_| Failure::Detached)?;
    let root = join(staging, ROOT);
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut done = 0u64;
    let mut result = files.make_directory(root.as_bytes()).map_err(failure);
    for file in &component.files {
        if result.is_err() {
            break;
        }
        result = copy_file(files, source, file, &root, &mut buffer, &mut |piece| {
            done += piece;
            progress(done, total)
        })
        .map_err(|failure| match failure {
            Failure::Damaged(_) => {
                Failure::Damaged([component.name.as_str(), ".component"].concat())
            }
            other => other,
        });
    }
    files.close(source);
    result?;
    let mut record = String::new();
    component.write_block(&mut record);
    let path = join(staging, RECORD);
    let handle = files.create(path.as_bytes(), 0o644).map_err(failure)?;
    let written = files
        .write(handle, record.as_bytes())
        .and_then(|()| files.sync_file(handle));
    files.close(handle);
    written.map_err(written_failure)
}

fn written_failure(errno: Errno) -> Failure {
    match errno {
        Errno::ENOSPC => Failure::NoSpace { needed: 0, free: 0 },
        errno => failure(errno),
    }
}

fn copy_file<F: Files>(
    files: &mut F,
    source: Handle,
    file: &index::File,
    root: &str,
    buffer: &mut [u8],
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<(), Failure> {
    let mut folder = String::from(root);
    let parts: Vec<&str> = file.path.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        folder.push('/');
        folder.push_str(part);
        match files.make_directory(folder.as_bytes()) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(errno) => return Err(failure(errno)),
        }
        if files.status(folder.as_bytes()).map(|status| status.kind) != Ok(Kind::Directory) {
            return Err(Failure::UnsafeFolder);
        }
    }
    let target = join(root, &file.path);
    let handle = files
        .create(target.as_bytes(), file.mode)
        .map_err(failure)?;
    let copied = copy_bytes(files, source, handle, file, buffer, progress);
    files.close(handle);
    copied
}

fn copy_bytes<F: Files>(
    files: &mut F,
    source: Handle,
    target: Handle,
    file: &index::File,
    buffer: &mut [u8],
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<(), Failure> {
    let mut hash = Sha256::new();
    let mut at = 0u64;
    while at < file.length {
        let want = (file.length - at).min(buffer.len() as u64) as usize;
        let read = match files.read_at(source, file.offset + at, &mut buffer[..want]) {
            Ok(0) | Err(_) => return Err(Failure::Detached),
            Ok(read) => read,
        };
        hash.update(&buffer[..read]);
        files
            .write(target, &buffer[..read])
            .map_err(written_failure)?;
        at += read as u64;
        if !progress(read as u64) {
            return Err(Failure::Cancelled);
        }
    }
    if hash.finish() != file.sha256 {
        return Err(Failure::Damaged(String::new()));
    }
    files.sync_file(target).map_err(written_failure)
}

/// Removes the installed component `name`: its folder is renamed away first, so it is no longer
/// installed even if deleting its files is cut short (the next action finishes that).
pub fn remove<F: Files>(files: &mut F, name: &str) -> Result<(), Failure> {
    tidy(files);
    let folder = component_folder(name);
    let removing = join(COMPONENTS, &[REMOVING_PREFIX, name].concat());
    files
        .rename(folder.as_bytes(), removing.as_bytes())
        .map_err(failure)?;
    files.sync();
    let deleted = remove_tree(files, &removing, 0);
    files.sync();
    deleted.map_err(Failure::FilesLeft)
}

/// Removes the installed component `name` once the person confirmed, as every front-end does:
/// takes the lock for `title`, checks the remove again against the components installed now,
/// removes it and gives the lock back. `Ok(None)` when it is no longer installed.
pub fn remove_checked<F: Files>(
    files: &mut F,
    title: &str,
    name: &str,
    pid: u32,
) -> Result<Option<Component>, RemoveError> {
    lock(files, title, true, pid).map_err(RemoveError::Refused)?;
    let installed = installed(files);
    let removed = match check_remove(files, &installed, name) {
        Ok(Some(component)) => remove(files, name)
            .map(|()| Some(component))
            .map_err(RemoveError::Failed),
        Ok(None) => Ok(None),
        Err(refusal) => Err(RemoveError::Refused(refusal)),
    };
    unlock(files);
    removed
}

fn remove_tree<F: Files>(files: &mut F, path: &str, depth: usize) -> Result<(), Errno> {
    let status = files.status(path.as_bytes())?;
    if status.kind != Kind::Directory {
        return files.remove_file(path.as_bytes());
    }
    if depth == MAX_DEPTH {
        return Err(Errno::ELOOP);
    }
    let mut name = [0u8; NAME_BYTES];
    while let Some(length) = files.entry(path.as_bytes(), 0, &mut name)? {
        let child = join(path, &String::from_utf8_lossy(&name[..length]));
        remove_tree(files, &child, depth + 1)?;
    }
    files.remove_directory(path.as_bytes())
}
