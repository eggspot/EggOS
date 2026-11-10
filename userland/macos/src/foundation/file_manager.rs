//! NSFileManager and the search-path functions (D232): whether a file exists and is a folder,
//! a folder's contents, making folders (with their parents), making a file, removing,
//! moving and copying items (folders as whole trees, walked without recursion), an item's
//! attributes, and where the standard folders are (`NSSearchPathForDirectoriesInDomains`,
//! `NSHomeDirectory`, `NSTemporaryDirectory`). Failures give the `NSCocoaErrorDomain` codes
//! macOS gives, with the POSIX error underneath and the path.

/// A domain of `NSSearchPathDomainMask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    User,
    Local,
    System,
}

const DOMAINS: [(usize, Option<Domain>); 4] = [
    (1, Some(Domain::User)),
    (2, Some(Domain::Local)),
    (4, None),
    (8, Some(Domain::System)),
];

/// Where `directory` (an `NSSearchPathDirectory`) is in `domain`: below the home folder for
/// the user domain, an absolute path for the others; `None` where macOS has none.
pub fn folder(directory: usize, domain: Domain) -> Option<&'static [u8]> {
    match (directory, domain) {
        (1, Domain::User) => Some(b"Applications"),
        (1, Domain::Local) => Some(b"/Applications"),
        (1, Domain::System) => Some(b"/System/Applications"),
        (5, Domain::User) => Some(b"Library"),
        (5, Domain::Local) => Some(b"/Library"),
        (5, Domain::System) => Some(b"/System/Library"),
        (9, Domain::User) => Some(b"Documents"),
        (12, Domain::User) => Some(b"Desktop"),
        (13, Domain::User) => Some(b"Library/Caches"),
        (13, Domain::Local) => Some(b"/Library/Caches"),
        (14, Domain::User) => Some(b"Library/Application Support"),
        (14, Domain::Local) => Some(b"/Library/Application Support"),
        (15, Domain::User) => Some(b"Downloads"),
        (17, Domain::User) => Some(b"Movies"),
        (18, Domain::User) => Some(b"Music"),
        (19, Domain::User) => Some(b"Pictures"),
        _ => None,
    }
}

/// The domains of `mask` that have `directory`, in the order macOS lists them.
pub fn domains(directory: usize, mask: usize) -> impl Iterator<Item = (Domain, &'static [u8])> {
    DOMAINS.iter().filter_map(move |&(bit, domain)| {
        let domain = domain.filter(|_| mask & bit != 0)?;
        Some((domain, folder(directory, domain)?))
    })
}

/// The `NSCocoaErrorDomain` code macOS gives for the Linux error `errno` when reading
/// (`reading`) or changing an item.
pub fn cocoa_code(errno: u16, reading: bool) -> isize {
    match (errno, reading) {
        (2, true) => 260,
        (2, false) => 4,
        (17, _) => 516,
        (1 | 13, true) => 257,
        (1 | 13, false) => 513,
        (28, _) => 640,
        (30, _) => 642,
        (_, true) => 256,
        (_, false) => 512,
    }
}

#[cfg(eggos_user)]
pub use manager::{define, resolve};

#[cfg(eggos_user)]
mod manager {
    use super::super::constants::{
        self, COCOA_ERROR_DOMAIN, FILE_ATTRIBUTE_KEYS, FILE_PATH_ERROR_KEY,
        FIRST_FILE_ATTRIBUTE_KEY, FIRST_FILE_TYPE, POSIX_ERROR_DOMAIN, UNDERLYING_ERROR_KEY,
    };
    use super::super::data::bytes_of;
    use super::super::date::{REFERENCE_FROM_1970, new_date};
    use super::super::{
        Definition, NSDATE, NSDICTIONARY, NSERROR, NSFILEMANAGER, NSFILEMANAGER_META,
        NSMUTABLEARRAY, NSNUMBER, autoreleased, class_pointer, define as define_class, is_data,
        new_instance, string_from_utf8, with_utf8,
    };
    use super::{Domain, cocoa_code, domains};
    use crate::darwin::{self, LINUX_STAT_SIZE, LinuxLayout, Status};
    use crate::libsystem::{heap, process};
    use crate::linux::{self, AT_FDCWD, AT_REMOVEDIR, AT_SYMLINK_NOFOLLOW, number};
    use crate::objc::abi::{Id, Sel};
    use crate::objc::buffer::Buffer;
    use crate::objc::message;
    use crate::objc::root::NSOBJECT;
    use crate::objc::selector::name_bytes;
    use crate::objc::{Runtime, methods, sel, send, send1};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use eggos_runtime::errno::Errno;

    const LONGEST_PATH: usize = 4096;
    const DIRECTORY_BUFFER: usize = 4096;
    const COPY_BUFFER: usize = 64 * 1024;
    const DEEPEST_COPY: usize = 64;
    const KIND_MASK: u32 = 0o170_000;
    const KIND_DIRECTORY: u32 = 0o040_000;
    const KIND_REGULAR: u32 = 0o100_000;
    const KIND_LINK: u32 = 0o120_000;
    const KIND_CHARACTER: u32 = 0o020_000;
    const KIND_BLOCK: u32 = 0o060_000;
    const KIND_SOCKET: u32 = 0o140_000;
    const PERMISSIONS: u32 = 0o7777;
    const NEW_FOLDER_MODE: usize = 0o777;
    const NEW_FILE_MODE: usize = 0o644;
    const READABLE: usize = 4;
    const WRITABLE: usize = 2;
    const EXECUTABLE: usize = 1;
    const TILDE: &[u8] = b"~";
    const WRITE_ONLY: u32 = 1;
    const DEFAULT_TEMPORARY: &[u8] = b"/tmp/";

    static DEFAULT_MANAGER: AtomicUsize = AtomicUsize::new(0);

    struct Path {
        bytes: [u8; LONGEST_PATH],
        length: usize,
    }

    impl Path {
        fn new(text: &[u8]) -> Option<Self> {
            let mut path = Self {
                bytes: [0; LONGEST_PATH],
                length: 0,
            };
            (!text.is_empty() && !text.contains(&0) && path.push_bytes(text)).then_some(path)
        }

        fn of(string: Id) -> Option<Self> {
            if string.is_null() {
                return None;
            }
            with_utf8(string, Self::new)
        }

        fn push_bytes(&mut self, more: &[u8]) -> bool {
            let end = self.length + more.len();
            if end >= LONGEST_PATH {
                return false;
            }
            self.bytes[self.length..end].copy_from_slice(more);
            self.bytes[end] = 0;
            self.length = end;
            true
        }

        fn push(&mut self, name: &[u8]) -> bool {
            let saved = self.length;
            let joined =
                (self.text().ends_with(b"/") || self.push_bytes(b"/")) && self.push_bytes(name);
            if !joined {
                self.truncate(saved);
            }
            joined
        }

        fn truncate(&mut self, length: usize) {
            self.length = length;
            self.bytes[length] = 0;
        }

        fn pop(&mut self) {
            match self.text().iter().rposition(|&byte| byte == b'/') {
                Some(0) => self.truncate(1),
                Some(at) => self.truncate(at),
                None => self.truncate(0),
            }
        }

        fn text(&self) -> &[u8] {
            &self.bytes[..self.length]
        }

        fn with_nul(&self) -> &[u8] {
            &self.bytes[..=self.length]
        }
    }

    fn status(path: &Path, follow: bool) -> Result<Status, Errno> {
        let mut raw = [0u8; LINUX_STAT_SIZE];
        let flags = if follow { 0 } else { AT_SYMLINK_NOFOLLOW };
        linux::stat_at(path.with_nul(), flags, &mut raw)?;
        Ok(Status::from_linux(&raw, LinuxLayout::HOST))
    }

    fn is_folder(status: &Status) -> bool {
        status.mode & KIND_MASK == KIND_DIRECTORY
    }

    fn make_folder(path: &Path, mode: usize) -> Result<(), Errno> {
        // SAFETY: `mkdirat` reads the NUL-terminated path.
        unsafe {
            linux::syscall(
                number::MKDIRAT,
                [AT_FDCWD, path.with_nul().as_ptr() as usize, mode, 0, 0, 0],
            )
        }
        .map(drop)
    }

    fn remove_entry(path: &Path, folder: bool) -> Result<(), Errno> {
        let flags = if folder { AT_REMOVEDIR } else { 0 };
        // SAFETY: `unlinkat` reads the NUL-terminated path.
        unsafe {
            linux::syscall(
                number::UNLINKAT,
                [AT_FDCWD, path.with_nul().as_ptr() as usize, flags, 0, 0, 0],
            )
        }
        .map(drop)
    }

    fn rename(from: &Path, to: &Path) -> Result<(), Errno> {
        // SAFETY: `renameat` reads the two NUL-terminated paths.
        unsafe {
            linux::syscall(
                number::RENAMEAT,
                [
                    AT_FDCWD,
                    from.with_nul().as_ptr() as usize,
                    AT_FDCWD,
                    to.with_nul().as_ptr() as usize,
                    0,
                    0,
                ],
            )
        }
        .map(drop)
    }

    fn accessible(path: &Path, mode: usize) -> bool {
        // SAFETY: `faccessat` reads the NUL-terminated path.
        unsafe {
            linux::syscall(
                number::FACCESSAT,
                [AT_FDCWD, path.with_nul().as_ptr() as usize, mode, 0, 0, 0],
            )
        }
        .is_ok()
    }

    #[derive(Clone, Copy)]
    struct Folder {
        fd: usize,
        buffer: *mut u8,
        at: usize,
        end: usize,
    }

    impl Folder {
        fn open(path: &Path) -> Result<Self, Errno> {
            let fd = linux::open(
                path.with_nul(),
                linux_flags(darwin::open::DIRECTORY | darwin::open::CLOEXEC)?,
                0,
            )?;
            let buffer = heap::allocate(DIRECTORY_BUFFER, false);
            if buffer.is_null() {
                let _ = linux::close(fd);
                return Err(Errno::ENOMEM);
            }
            Ok(Self {
                fd,
                buffer,
                at: 0,
                end: 0,
            })
        }

        fn next(&mut self) -> Result<Option<&[u8]>, Errno> {
            loop {
                if self.at >= self.end {
                    // SAFETY: `getdents64` writes at most the buffer's size into it.
                    let read = unsafe {
                        linux::syscall(
                            number::GETDENTS64,
                            [self.fd, self.buffer as usize, DIRECTORY_BUFFER, 0, 0, 0],
                        )
                    }?;
                    if read == 0 {
                        return Ok(None);
                    }
                    self.at = 0;
                    self.end = read.min(DIRECTORY_BUFFER);
                }
                // SAFETY: the block holds `DIRECTORY_BUFFER` bytes, the kernel filled `end`,
                // and they stay until the next read, which needs `self` borrowed again.
                let filled: &[u8] = unsafe { core::slice::from_raw_parts(self.buffer, self.end) };
                let Some((_, _, _, name, record)) = darwin::linux_dirent(&filled[self.at..]) else {
                    return Err(Errno::EIO);
                };
                if record == 0 {
                    return Err(Errno::EIO);
                }
                self.at += record;
                if name != b"." && name != b".." {
                    return Ok(Some(name));
                }
            }
        }

        fn close(self) {
            let _ = linux::close(self.fd);
            heap::release(self.buffer);
        }
    }

    fn linux_flags(flags: u32) -> Result<usize, Errno> {
        darwin::linux_open_flags(flags, LinuxLayout::HOST)
            .map(|flags| flags as usize)
            .ok_or(Errno::EINVAL)
    }

    fn remove_tree(path: &mut Path) -> Result<(), Errno> {
        let root = path.length;
        loop {
            if is_folder(&status(path, false)?) {
                let mut folder = Folder::open(path)?;
                let pushed = match folder.next() {
                    Ok(Some(name)) => Ok(Some(path.push(name))),
                    Ok(None) => Ok(None),
                    Err(errno) => Err(errno),
                };
                folder.close();
                match pushed? {
                    Some(true) => continue,
                    Some(false) => return Err(Errno::ENAMETOOLONG),
                    None => remove_entry(path, true)?,
                }
            } else {
                remove_entry(path, false)?;
            }
            if path.length <= root {
                return Ok(());
            }
            path.pop();
        }
    }

    fn copy_file(source: &Path, target: &Path, mode: u32) -> Result<(), Errno> {
        let input = linux::open(source.with_nul(), linux::O_RDONLY | linux::O_CLOEXEC, 0)?;
        let flags = linux_flags(
            WRITE_ONLY | darwin::open::CREAT | darwin::open::EXCL | darwin::open::CLOEXEC,
        );
        let output = match flags
            .and_then(|flags| linux::open(target.with_nul(), flags, (mode & PERMISSIONS) as usize))
        {
            Ok(fd) => fd,
            Err(errno) => {
                let _ = linux::close(input);
                return Err(errno);
            }
        };
        let buffer = heap::allocate(COPY_BUFFER, false);
        let result = match buffer.is_null() {
            true => Err(Errno::ENOMEM),
            false => {
                // SAFETY: the new block holds `COPY_BUFFER` bytes.
                let chunk = unsafe { core::slice::from_raw_parts_mut(buffer, COPY_BUFFER) };
                loop {
                    match linux::read(input, chunk) {
                        Ok(0) => break Ok(()),
                        Ok(read) => {
                            if let Err(errno) = linux::write_all(output, &chunk[..read]) {
                                break Err(errno);
                            }
                        }
                        Err(errno) => break Err(errno),
                    }
                }
            }
        };
        heap::release(buffer);
        let _ = linux::close(input);
        let _ = linux::close(output);
        result
    }

    fn copy_entry(
        source: &Path,
        target: &Path,
        folders: &mut Buffer<Folder>,
    ) -> Result<bool, Errno> {
        let found = status(source, false)?;
        match found.mode & KIND_MASK {
            KIND_DIRECTORY => {
                if folders.len() >= DEEPEST_COPY {
                    return Err(Errno::ELOOP);
                }
                make_folder(target, (found.mode & PERMISSIONS) as usize)?;
                let folder = Folder::open(source)?;
                if !folders.push(folder) {
                    folder.close();
                    return Err(Errno::ENOMEM);
                }
                Ok(true)
            }
            KIND_REGULAR => copy_file(source, target, found.mode).map(|()| false),
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn copy_tree(source: &mut Path, target: &mut Path) -> Result<(), Errno> {
        let mut folders = Buffer::new();
        let mut result = copy_entry(source, target, &mut folders).map(drop);
        while result.is_ok() {
            let Some(top) = folders.as_mut_slice().last_mut() else {
                break;
            };
            let pushed = match top.next() {
                Ok(Some(name)) => Ok(Some(source.push(name) && target.push(name))),
                Ok(None) => Ok(None),
                Err(errno) => Err(errno),
            };
            result = match pushed {
                Err(errno) => Err(errno),
                Ok(Some(false)) => Err(Errno::ENAMETOOLONG),
                Ok(Some(true)) => match copy_entry(source, target, &mut folders) {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        source.pop();
                        target.pop();
                        Ok(())
                    }
                    Err(errno) => Err(errno),
                },
                Ok(None) => {
                    if let Some(done) = folders.pop() {
                        done.close();
                    }
                    if !folders.as_slice().is_empty() {
                        source.pop();
                        target.pop();
                    }
                    Ok(())
                }
            };
        }
        while let Some(folder) = folders.pop() {
            folder.close();
        }
        folders.free();
        result
    }

    fn string(bytes: &[u8]) -> Id {
        string_from_utf8(bytes)
    }

    fn dictionary(keys: &[Id], objects: &[Id]) -> Id {
        let make: extern "C" fn(Id, Sel, *const Id, *const Id, usize) -> Id =
            message::msg_send_as();
        make(
            class_pointer(&NSDICTIONARY).cast(),
            sel!("dictionaryWithObjects:forKeys:count:"),
            objects.as_ptr(),
            keys.as_ptr(),
            keys.len().min(objects.len()),
        )
    }

    fn new_error(domain: usize, code: isize, user_info: Id) -> Id {
        let make: extern "C" fn(Id, Sel, Id, isize, Id) -> Id = message::msg_send_as();
        make(
            class_pointer(&NSERROR).cast(),
            sel!("errorWithDomain:code:userInfo:"),
            constants::string(domain),
            code,
            user_info,
        )
    }

    fn fail(error: *mut Id, errno: Errno, path: Id, reading: bool) -> u8 {
        if error.is_null() {
            return 0;
        }
        let posix = darwin::darwin_errno(errno.0) as isize;
        let underlying = new_error(POSIX_ERROR_DOMAIN, posix, core::ptr::null_mut());
        let mut keys = [
            constants::string(UNDERLYING_ERROR_KEY),
            core::ptr::null_mut(),
        ];
        let mut objects = [underlying, core::ptr::null_mut()];
        let mut count = 1;
        if !path.is_null() {
            keys[1] = constants::string(FILE_PATH_ERROR_KEY);
            objects[1] = path;
            count = 2;
        }
        let user_info = dictionary(&keys[..count], &objects[..count]);
        let made = new_error(COCOA_ERROR_DOMAIN, cocoa_code(errno.0, reading), user_info);
        // SAFETY: the caller's out-parameter for the error.
        unsafe { error.write(made) };
        0
    }

    fn outcome(result: Result<(), Errno>, error: *mut Id, path: Id, reading: bool) -> u8 {
        match result {
            Ok(()) => 1,
            Err(errno) => fail(error, errno, path, reading),
        }
    }

    extern "C" fn default_manager(_: Id, _: Sel) -> Id {
        let existing = DEFAULT_MANAGER.load(Ordering::Acquire);
        if existing != 0 {
            return existing as Id;
        }
        let made = new_instance(class_pointer(&NSFILEMANAGER));
        match DEFAULT_MANAGER.compare_exchange(
            0,
            made as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => made,
            Err(winner) => {
                crate::objc::reference_count::dispose(made);
                winner as Id
            }
        }
    }

    extern "C" fn file_exists(_: Id, _: Sel, path: Id) -> u8 {
        u8::from(Path::of(path).is_some_and(|path| status(&path, true).is_ok()))
    }

    unsafe extern "C" fn file_exists_is_folder(_: Id, _: Sel, path: Id, folder: *mut u8) -> u8 {
        let found = Path::of(path).and_then(|path| status(&path, true).ok());
        if let (Some(found), false) = (&found, folder.is_null()) {
            // SAFETY: the caller's out-parameter for whether it is a folder.
            unsafe { folder.write(u8::from(is_folder(found))) };
        }
        u8::from(found.is_some())
    }

    fn access_check(path: Id, mode: usize) -> u8 {
        u8::from(Path::of(path).is_some_and(|path| accessible(&path, mode)))
    }

    extern "C" fn is_readable(_: Id, _: Sel, path: Id) -> u8 {
        access_check(path, READABLE)
    }

    extern "C" fn is_writable(_: Id, _: Sel, path: Id) -> u8 {
        access_check(path, WRITABLE)
    }

    extern "C" fn is_executable(_: Id, _: Sel, path: Id) -> u8 {
        access_check(path, EXECUTABLE)
    }

    extern "C" fn is_deletable(_: Id, _: Sel, path: Id) -> u8 {
        let Some(mut parent) = Path::of(path) else {
            return 0;
        };
        parent.pop();
        if parent.length == 0 && !parent.push_bytes(b".") {
            return 0;
        }
        u8::from(accessible(&parent, WRITABLE | EXECUTABLE))
    }

    fn contents_of(path: &Path) -> Result<Id, Errno> {
        let array = send(class_pointer(&NSMUTABLEARRAY).cast(), sel!("array"));
        let mut folder = Folder::open(path)?;
        let result = loop {
            match folder.next() {
                Ok(Some(name)) => {
                    send1(array, sel!("addObject:"), string(name) as usize);
                }
                Ok(None) => break Ok(array),
                Err(errno) => break Err(errno),
            }
        };
        folder.close();
        result
    }

    unsafe extern "C" fn contents_of_folder(_: Id, _: Sel, path: Id, error: *mut Id) -> Id {
        let result = Path::of(path)
            .ok_or(Errno::ENOENT)
            .and_then(|at| contents_of(&at));
        match result {
            Ok(array) => autoreleased(send(array, sel!("copy"))),
            Err(errno) => {
                fail(error, errno, path, true);
                core::ptr::null_mut()
            }
        }
    }

    fn make_folders(path: &mut Path, parents: bool) -> Result<(), Errno> {
        if !parents {
            return make_folder(path, NEW_FOLDER_MODE);
        }
        let full = path.length;
        let mut end = 1;
        while end <= full {
            let at_separator = end == full || path.bytes[end] == b'/';
            if at_separator {
                let saved = path.bytes[end];
                path.bytes[end] = 0;
                path.length = end;
                let made = make_folder(path, NEW_FOLDER_MODE);
                path.bytes[end] = saved;
                path.length = full;
                match made {
                    Ok(()) => {}
                    Err(Errno::EEXIST) => {
                        let prefix = Path::new(&path.bytes[..end]).ok_or(Errno::EINVAL)?;
                        if !is_folder(&status(&prefix, true)?) {
                            return Err(Errno::ENOTDIR);
                        }
                    }
                    Err(errno) => return Err(errno),
                }
            }
            end += 1;
        }
        Ok(())
    }

    unsafe extern "C" fn create_folder(
        _: Id,
        _: Sel,
        path: Id,
        parents: u8,
        _attributes: Id,
        error: *mut Id,
    ) -> u8 {
        let result = Path::of(path)
            .ok_or(Errno::EINVAL)
            .and_then(|mut at| make_folders(&mut at, parents != 0));
        outcome(result, error, path, false)
    }

    fn write_file(path: &Path, contents: &[u8]) -> Result<(), Errno> {
        let flags = linux_flags(
            WRITE_ONLY | darwin::open::CREAT | darwin::open::TRUNC | darwin::open::CLOEXEC,
        )?;
        let fd = linux::open(path.with_nul(), flags, NEW_FILE_MODE)?;
        let written = linux::write_all(fd, contents);
        let _ = linux::close(fd);
        written
    }

    extern "C" fn create_file(_: Id, _: Sel, path: Id, contents: Id, _attributes: Id) -> u8 {
        let bytes = match !contents.is_null() && is_data(contents) {
            true => bytes_of(contents),
            false => &[][..],
        };
        u8::from(Path::of(path).is_some_and(|at| write_file(&at, bytes).is_ok()))
    }

    unsafe extern "C" fn remove_item(_: Id, _: Sel, path: Id, error: *mut Id) -> u8 {
        let result = Path::of(path)
            .ok_or(Errno::ENOENT)
            .and_then(|mut at| remove_tree(&mut at));
        outcome(result, error, path, false)
    }

    fn move_item(from: &Path, to: &Path) -> Result<(), Errno> {
        if status(to, false).is_ok() {
            return Err(Errno::EEXIST);
        }
        status(from, false)?;
        rename(from, to)
    }

    unsafe extern "C" fn move_item_to(_: Id, _: Sel, from: Id, to: Id, error: *mut Id) -> u8 {
        let result = match (Path::of(from), Path::of(to)) {
            (Some(source), Some(target)) => move_item(&source, &target),
            _ => Err(Errno::EINVAL),
        };
        outcome(result, error, to, false)
    }

    fn copy_item(source: &mut Path, target: &mut Path) -> Result<(), Errno> {
        if status(target, false).is_ok() {
            return Err(Errno::EEXIST);
        }
        copy_tree(source, target)
    }

    unsafe extern "C" fn copy_item_to(_: Id, _: Sel, from: Id, to: Id, error: *mut Id) -> u8 {
        let result = match (Path::of(from), Path::of(to)) {
            (Some(mut source), Some(mut target)) => copy_item(&mut source, &mut target),
            _ => Err(Errno::EINVAL),
        };
        outcome(result, error, to, false)
    }

    fn file_type(mode: u32) -> Id {
        let offset = match mode & KIND_MASK {
            KIND_REGULAR => 0,
            KIND_DIRECTORY => 1,
            KIND_LINK => 2,
            KIND_CHARACTER => 3,
            KIND_BLOCK => 4,
            KIND_SOCKET => 5,
            _ => 6,
        };
        constants::string(FIRST_FILE_TYPE + offset)
    }

    fn unsigned_number(value: u64) -> Id {
        send1(
            class_pointer(&NSNUMBER).cast(),
            sel!("numberWithUnsignedLongLong:"),
            value as usize,
        )
    }

    fn attributes(found: &Status) -> Id {
        let modified = found.times[1];
        let seconds = modified[0] as f64 + modified[1] as f64 / 1e9 - REFERENCE_FROM_1970;
        let date = autoreleased(new_date(class_pointer(&NSDATE), seconds));
        let objects: [Id; FILE_ATTRIBUTE_KEYS] = [
            unsigned_number(found.size.max(0) as u64),
            file_type(found.mode),
            date,
            unsigned_number(u64::from(found.mode & PERMISSIONS)),
            unsigned_number(found.links),
            unsigned_number(u64::from(found.user)),
            unsigned_number(u64::from(found.group)),
            unsigned_number(found.device),
            unsigned_number(found.inode),
        ];
        let keys: [Id; FILE_ATTRIBUTE_KEYS] =
            core::array::from_fn(|index| constants::string(FIRST_FILE_ATTRIBUTE_KEY + index));
        dictionary(&keys, &objects)
    }

    unsafe extern "C" fn attributes_of_item(_: Id, _: Sel, path: Id, error: *mut Id) -> Id {
        let result = Path::of(path)
            .ok_or(Errno::ENOENT)
            .and_then(|at| status(&at, false));
        match result {
            Ok(found) => attributes(&found),
            Err(errno) => {
                fail(error, errno, path, true);
                core::ptr::null_mut()
            }
        }
    }

    extern "C" fn current_folder(_: Id, _: Sel) -> Id {
        let mut buffer = [0u8; LONGEST_PATH];
        // SAFETY: `getcwd` writes at most the buffer's size into it.
        match unsafe {
            linux::syscall(
                number::GETCWD,
                [buffer.as_mut_ptr() as usize, LONGEST_PATH, 0, 0, 0, 0],
            )
        } {
            Ok(_) => {
                let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(0);
                string(&buffer[..end])
            }
            Err(_) => core::ptr::null_mut(),
        }
    }

    extern "C" fn change_current_folder(_: Id, _: Sel, path: Id) -> u8 {
        let Some(at) = Path::of(path) else {
            return 0;
        };
        // SAFETY: `chdir` reads the NUL-terminated path.
        let changed = unsafe {
            linux::syscall(
                number::CHDIR,
                [at.with_nul().as_ptr() as usize, 0, 0, 0, 0, 0],
            )
        };
        u8::from(changed.is_ok())
    }

    fn environment(name: &core::ffi::CStr) -> Option<&'static [u8]> {
        // SAFETY: `getenv` reads the NUL-terminated name.
        let value = unsafe { process::getenv(name.as_ptr()) };
        if value.is_null() {
            return None;
        }
        // SAFETY: an environment value is a NUL-terminated string that lives as long as the
        // program.
        let bytes = unsafe { name_bytes(value.cast_const().cast()) };
        (!bytes.is_empty()).then_some(bytes)
    }

    fn home() -> &'static [u8] {
        environment(c"HOME").unwrap_or(b"/")
    }

    extern "C" fn ns_home_directory() -> Id {
        string(home())
    }

    extern "C" fn ns_temporary_directory() -> Id {
        let folder = environment(c"TMPDIR").unwrap_or(DEFAULT_TEMPORARY);
        let mut path = Path::new(folder).unwrap_or_else(|| {
            Path::new(DEFAULT_TEMPORARY).unwrap_or(Path {
                bytes: [0; LONGEST_PATH],
                length: 0,
            })
        });
        if !path.text().ends_with(b"/") {
            path.push_bytes(b"/");
        }
        string(path.text())
    }

    extern "C" fn ns_search_path(directory: usize, mask: usize, expand_tilde: u8) -> Id {
        let array = send(class_pointer(&NSMUTABLEARRAY).cast(), sel!("array"));
        for (domain, folder) in domains(directory, mask) {
            let made = match domain {
                Domain::User => {
                    let base = if expand_tilde != 0 { home() } else { TILDE };
                    Path::new(base).and_then(|mut path| path.push(folder).then_some(path))
                }
                Domain::Local | Domain::System => Path::new(folder),
            };
            if let Some(path) = made {
                send1(array, sel!("addObject:"), string(path.text()) as usize);
            }
        }
        autoreleased(send(array, sel!("copy")))
    }

    extern "C" fn this_object(this: Id, _: Sel) -> Id {
        this
    }

    extern "C" fn nothing(_: Id, _: Sel) {}

    pub fn define(runtime: &mut Runtime) -> bool {
        let instance = methods![
            "fileExistsAtPath:" => file_exists,
            "fileExistsAtPath:isDirectory:" => file_exists_is_folder,
            "isReadableFileAtPath:" => is_readable,
            "isWritableFileAtPath:" => is_writable,
            "isExecutableFileAtPath:" => is_executable,
            "isDeletableFileAtPath:" => is_deletable,
            "contentsOfDirectoryAtPath:error:" => contents_of_folder,
            "createDirectoryAtPath:withIntermediateDirectories:attributes:error:" => create_folder,
            "createFileAtPath:contents:attributes:" => create_file,
            "removeItemAtPath:error:" => remove_item,
            "moveItemAtPath:toPath:error:" => move_item_to,
            "copyItemAtPath:toPath:error:" => copy_item_to,
            "attributesOfItemAtPath:error:" => attributes_of_item,
            "currentDirectoryPath" => current_folder,
            "changeCurrentDirectoryPath:" => change_current_folder,
            "retain" => this_object,
            "release" => nothing,
            "autorelease" => this_object,
        ];
        let class = methods!["defaultManager" => default_manager];
        define_class(
            runtime,
            &Definition {
                name: b"NSFileManager\0",
                class: &NSFILEMANAGER,
                meta: &NSFILEMANAGER_META,
                superclass: &NSOBJECT,
                instance_size: core::mem::size_of::<usize>(),
                flags: 0,
                instance_methods: &instance,
                class_methods: &class,
            },
        )
    }

    /// The address of the search-path function `name`, if it is one of these.
    pub fn resolve(name: &[u8]) -> Option<u64> {
        let address = match name {
            b"_NSHomeDirectory" => ns_home_directory as *const () as usize,
            b"_NSTemporaryDirectory" => ns_temporary_directory as *const () as usize,
            b"_NSSearchPathForDirectoriesInDomains" => ns_search_path as *const () as usize,
            _ => return None,
        };
        Some(address as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_paths_follow_the_domain_order() {
        let all: Vec<_> = domains(5, 0xffff).collect();
        assert_eq!(
            all,
            [
                (Domain::User, &b"Library"[..]),
                (Domain::Local, &b"/Library"[..]),
                (Domain::System, &b"/System/Library"[..]),
            ]
        );
        let user: Vec<_> = domains(14, 1).collect();
        assert_eq!(user, [(Domain::User, &b"Library/Application Support"[..])]);
        assert_eq!(domains(9, 2).count(), 0);
        assert_eq!(domains(99, 0xffff).count(), 0);
        assert_eq!(domains(1, 4).count(), 0);
    }

    #[test]
    fn errors_get_the_codes_macos_gives() {
        assert_eq!(cocoa_code(2, true), 260);
        assert_eq!(cocoa_code(2, false), 4);
        assert_eq!(cocoa_code(17, false), 516);
        assert_eq!(cocoa_code(13, false), 513);
        assert_eq!(cocoa_code(13, true), 257);
        assert_eq!(cocoa_code(28, false), 640);
        assert_eq!(cocoa_code(5, true), 256);
        assert_eq!(cocoa_code(5, false), 512);
    }
}
