//! shell32's file operations (D178): `SHFileOperationW` copies, moves, renames and deletes
//! files and folders (folders whole, names with `*` and `?` expanded) with no window, a
//! delete with `FOF_ALLOWUNDO` moving them to the Trash, which is the Recycle Bin
//! (`crate::trash`, D135), and `SHEmptyRecycleBinW` empties it. Where Windows would ask the
//! user (a name already taken without `FOF_NOCONFIRMATION`, emptying the bin without
//! `SHERB_NOCONFIRMATION`) the operation is refused, as no question can be asked yet.
//! `ShellExecuteExW` starts programs as `CreateProcessW` does; documents, folders and URLs
//! have no application associated yet (`ERROR_NO_ASSOCIATION`).

use super::shell_folders::{S_OK, from_win32, home, make_folders};
use super::{Bool, FALSE, TRUE, error, error_for, fail, heap};
use crate::calendar;
use crate::find;
use crate::library::Export;
use crate::linux::{self, AT_FDCWD, O_CREAT, O_EXCL, O_WRONLY};
use crate::text::{self, PATH_CAPACITY};
use crate::trash;

const FO_MOVE: u32 = 1;
const FO_COPY: u32 = 2;
const FO_DELETE: u32 = 3;
const FO_RENAME: u32 = 4;
const FOF_MULTIDESTFILES: u16 = 0x1;
const FOF_NOCONFIRMATION: u16 = 0x10;
const FOF_ALLOWUNDO: u16 = 0x40;
const FOF_FILESONLY: u16 = 0x80;
const OPERATION_FUNCTION: usize = 8;
const OPERATION_FROM: usize = 16;
const OPERATION_TO: usize = 24;
const OPERATION_FLAGS: usize = 32;
const OPERATION_ABORTED: usize = 36;
const SHERB_NOCONFIRMATION: u32 = 0x1;
const EXDEV: u16 = 18;
const EEXIST: u16 = 17;
const MAX_DEPTH: usize = 64;
const MAX_PASSES: usize = 8;
const MAX_TRASH_ATTEMPTS: u32 = 10_000;

fn exists(path: &[u8]) -> Option<bool> {
    linux::stat_at(AT_FDCWD, path)
        .ok()
        .map(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY)
}

fn child(parent: &[u8], name: &[u8], out: &mut [u8; PATH_CAPACITY]) -> Result<usize, u32> {
    let parent = &parent[..parent.len() - 1];
    let length = parent.len() + 1 + name.len();
    if length + 1 > out.len() {
        return Err(error::FILENAME_EXCED_RANGE);
    }
    out[..parent.len()].copy_from_slice(parent);
    out[parent.len()] = b'/';
    out[parent.len() + 1..length].copy_from_slice(name);
    out[length] = 0;
    Ok(length + 1)
}

fn last_name(path: &[u8]) -> &[u8] {
    let path = &path[..path.len() - 1];
    path.rsplit(|byte| *byte == b'/').next().unwrap_or(path)
}

fn remove_tree(path: &[u8], depth: usize) -> Result<(), u32> {
    match exists(path) {
        None => return Err(error::FILE_NOT_FOUND),
        Some(false) => {
            return linux::remove(path, 0).map(|_| ()).map_err(error_for);
        }
        Some(true) => {}
    }
    if depth >= MAX_DEPTH {
        return Err(error::FILENAME_EXCED_RANGE);
    }
    remove_children(path, depth)?;
    linux::remove(path, linux::AT_REMOVEDIR)
        .map(|_| ())
        .map_err(error_for)
}

fn remove_children(path: &[u8], depth: usize) -> Result<(), u32> {
    for _ in 0..MAX_PASSES {
        let mut failure = None;
        let mut seen = false;
        super::directory::for_each_entry(path, &mut |name, _| {
            seen = true;
            let mut inner = [0u8; PATH_CAPACITY];
            let result = child(path, name, &mut inner)
                .and_then(|length| remove_tree(&inner[..length], depth + 1));
            if let Err(code) = result {
                failure.get_or_insert(code);
            }
        });
        if let Some(code) = failure {
            return Err(code);
        }
        if !seen {
            return Ok(());
        }
    }
    Err(error::DIR_NOT_EMPTY)
}

fn copy_tree(from: &[u8], to: &[u8], replace: bool, depth: usize) -> Result<(), u32> {
    match exists(from) {
        None => Err(error::FILE_NOT_FOUND),
        Some(false) => super::file::copy_file_at(from, to, replace),
        Some(true) => {
            if depth >= MAX_DEPTH {
                return Err(error::FILENAME_EXCED_RANGE);
            }
            match linux::make_directory(to) {
                Ok(_) => {}
                Err(errno) if errno.0 == EEXIST && replace => {}
                Err(errno) if errno.0 == EEXIST => return Err(error::ALREADY_EXISTS),
                Err(errno) => return Err(error_for(errno)),
            }
            let mut failure = None;
            super::directory::for_each_entry(from, &mut |name, _| {
                let (mut source, mut target) = ([0u8; PATH_CAPACITY], [0u8; PATH_CAPACITY]);
                let result = child(from, name, &mut source).and_then(|source_length| {
                    let target_length = child(to, name, &mut target)?;
                    copy_tree(
                        &source[..source_length],
                        &target[..target_length],
                        replace,
                        depth + 1,
                    )
                });
                if let Err(code) = result {
                    failure.get_or_insert(code);
                }
            });
            failure.map_or(Ok(()), Err)
        }
    }
}

fn move_item(from: &[u8], to: &[u8], replace: bool) -> Result<(), u32> {
    match linux::rename(from, to, !replace) {
        Ok(_) => Ok(()),
        Err(errno) if errno.0 == EEXIST => Err(error::ALREADY_EXISTS),
        Err(errno) if errno.0 == EXDEV => {
            copy_tree(from, to, replace, 0)?;
            remove_tree(from, 0)
        }
        Err(errno) => Err(super::directory::path_error(from, errno)),
    }
}

fn now() -> calendar::SystemTime {
    let nanoseconds = linux::clock_nanoseconds(linux::CLOCK_REALTIME);
    let file_time =
        nanoseconds / 100 + calendar::UNIX_EPOCH_SECONDS * calendar::INTERVALS_PER_SECOND;
    calendar::system_time(file_time).unwrap_or_default()
}

fn trash_folder(part: &[u8], out: &mut [u8; PATH_CAPACITY]) -> Result<usize, u32> {
    let mut length = home(out);
    for piece in [&b"/"[..], trash::UNDER_HOME, &b"/"[..], part] {
        if length + piece.len() + 1 > out.len() {
            return Err(error::FILENAME_EXCED_RANGE);
        }
        out[length..length + piece.len()].copy_from_slice(piece);
        length += piece.len();
    }
    out[length] = 0;
    match make_folders(&out[..=length]) {
        true => Ok(length + 1),
        false => Err(error::ACCESS_DENIED),
    }
}

fn move_to_trash(path: &[u8]) -> Result<(), u32> {
    let mut directory = [0u8; PATH_CAPACITY];
    let directory_length =
        super::directory::working_directory(&mut directory).ok_or(error::NOT_ENOUGH_MEMORY)?;
    let mut original = [0u8; PATH_CAPACITY];
    let original_length = text::absolute_path(
        &directory[..directory_length],
        &path[..path.len() - 1],
        &mut original,
    )
    .ok_or(error::FILENAME_EXCED_RANGE)?;
    let (mut files, mut info) = ([0u8; PATH_CAPACITY], [0u8; PATH_CAPACITY]);
    let files_length = trash_folder(b"files", &mut files)?;
    let info_length = trash_folder(b"info", &mut info)?;
    let name = last_name(path);
    for attempt in 1..=MAX_TRASH_ATTEMPTS {
        let mut trashed = [0u8; 256];
        let Some(trashed_length) = trash::trashed_name(name, attempt, &mut trashed) else {
            return Err(error::FILENAME_EXCED_RANGE);
        };
        let mut record_name = [0u8; 270];
        record_name[..trashed_length].copy_from_slice(&trashed[..trashed_length]);
        record_name[trashed_length..trashed_length + trash::INFO_EXTENSION.len()]
            .copy_from_slice(trash::INFO_EXTENSION);
        let record_name = &record_name[..trashed_length + trash::INFO_EXTENSION.len()];
        let mut record_path = [0u8; PATH_CAPACITY];
        let record_length = child(&info[..info_length], record_name, &mut record_path)?;
        let record_path = &record_path[..record_length];
        let fd = match linux::open(record_path, O_WRONLY | O_CREAT | O_EXCL) {
            Ok(fd) => fd,
            Err(errno) if errno.0 == EEXIST => continue,
            Err(errno) => return Err(error_for(errno)),
        };
        let mut written = Ok(());
        trash::write_record(&original[..original_length], &now(), &mut |piece| {
            if written.is_ok() {
                written = linux::write_all(fd, piece).map_err(error_for);
            }
        });
        let _ = linux::close(fd);
        let mut target = [0u8; PATH_CAPACITY];
        let moved = written.and_then(|()| {
            let target_length = child(
                &files[..files_length],
                &trashed[..trashed_length],
                &mut target,
            )?;
            match linux::rename(path, &target[..target_length], true) {
                Ok(_) => Ok(()),
                Err(errno) => Err(super::directory::path_error(path, errno)),
            }
        });
        if moved.is_err() {
            let _ = linux::remove(record_path, 0);
        }
        return moved;
    }
    Err(error::ALREADY_EXISTS)
}

#[derive(Debug, Clone, Copy)]
struct Operation {
    function: u32,
    flags: u16,
}

impl Operation {
    fn apply(self, source: &[u8], target: Option<&[u8]>) -> Result<(), u32> {
        let replace = self.flags & FOF_NOCONFIRMATION != 0;
        match (self.function, target) {
            (FO_DELETE, _) if self.flags & FOF_ALLOWUNDO != 0 => move_to_trash(source),
            (FO_DELETE, _) => remove_tree(source, 0),
            (FO_COPY, Some(target)) => {
                if !replace && exists(target).is_some() {
                    return Err(error::ALREADY_EXISTS);
                }
                copy_tree(source, target, replace, 0)
            }
            (FO_MOVE | FO_RENAME, Some(target)) => move_item(source, target, replace),
            _ => Err(error::INVALID_PARAMETER),
        }
    }
}

fn target_for(
    destination: &[u8],
    name: &[u8],
    into_folder: bool,
    out: &mut [u8; PATH_CAPACITY],
) -> Result<usize, u32> {
    match into_folder {
        true => child(destination, name, out),
        false => {
            out[..destination.len()].copy_from_slice(destination);
            Ok(destination.len())
        }
    }
}

fn run_one(
    operation: Operation,
    source: &[u16],
    destination: Option<&[u16]>,
    many: bool,
) -> Result<(), u32> {
    let mut path = [0u8; PATH_CAPACITY];
    let length =
        text::eggos_path(text::utf16_chars(source), &mut path).ok_or(error::INVALID_PARAMETER)?;
    let path = &path[..length];
    let mut target = [0u8; PATH_CAPACITY];
    let target_length = match destination {
        Some(destination) => Some(
            text::eggos_path(text::utf16_chars(destination), &mut target)
                .ok_or(error::INVALID_PARAMETER)?,
        ),
        None => None,
    };
    let target = target_length.map(|length| &target[..length]);
    let (directory, pattern) = find::split(&path[..length - 1]);
    let wildcard = pattern.iter().any(|byte| *byte == b'*' || *byte == b'?');
    let into_folder = match (operation.function, target) {
        (FO_COPY | FO_MOVE, Some(target)) => {
            let folder = exists(target) == Some(true);
            if !folder && (many || wildcard) {
                if !make_folders(target) {
                    return Err(error::PATH_NOT_FOUND);
                }
                true
            } else {
                folder
            }
        }
        _ => false,
    };
    if !wildcard {
        if exists(path).is_none() {
            return Err(error::FILE_NOT_FOUND);
        }
        let mut item_target = [0u8; PATH_CAPACITY];
        let item_target = match target {
            Some(target) => {
                let length = target_for(target, last_name(path), into_folder, &mut item_target)?;
                Some(&item_target[..length])
            }
            None => None,
        };
        return operation.apply(path, item_target);
    }
    let mut folder = [0u8; PATH_CAPACITY];
    folder[..directory.len()].copy_from_slice(directory);
    let folder = &folder[..directory.len() + 1];
    let mut failure = None;
    let mut found = false;
    super::directory::for_each_entry(folder, &mut |name, is_folder| {
        if !find::matches(pattern, name) || (is_folder && operation.flags & FOF_FILESONLY != 0) {
            return;
        }
        found = true;
        let mut item = [0u8; PATH_CAPACITY];
        let mut item_target = [0u8; PATH_CAPACITY];
        let result = child(folder, name, &mut item).and_then(|length| {
            let target = match target {
                Some(target) => {
                    let target_length = target_for(target, name, into_folder, &mut item_target)?;
                    Some(&item_target[..target_length])
                }
                None => None,
            };
            operation.apply(&item[..length], target)
        });
        if let Err(code) = result {
            failure.get_or_insert(code);
        }
    });
    match (failure, found) {
        (Some(code), _) => Err(code),
        (None, false) => Err(error::FILE_NOT_FOUND),
        (None, true) => Ok(()),
    }
}

unsafe extern "efiapi" fn sh_file_operation_w(operation: *mut u8) -> i32 {
    if operation.is_null() {
        return error::INVALID_PARAMETER as i32;
    }
    // SAFETY: `SHFileOperationW`'s contract: a readable and writable `SHFILEOPSTRUCTW`.
    let function = unsafe {
        operation
            .wrapping_add(OPERATION_FUNCTION)
            .cast::<u32>()
            .read_unaligned()
    };
    // SAFETY: as above.
    let from = unsafe {
        operation
            .wrapping_add(OPERATION_FROM)
            .cast::<*const u16>()
            .read_unaligned()
    };
    // SAFETY: as above.
    let to = unsafe {
        operation
            .wrapping_add(OPERATION_TO)
            .cast::<*const u16>()
            .read_unaligned()
    };
    // SAFETY: as above.
    let flags = unsafe {
        operation
            .wrapping_add(OPERATION_FLAGS)
            .cast::<u16>()
            .read_unaligned()
    };
    // SAFETY: as above.
    unsafe {
        operation
            .wrapping_add(OPERATION_ABORTED)
            .cast::<Bool>()
            .write_unaligned(FALSE)
    };
    if from.is_null()
        || !(FO_MOVE..=FO_RENAME).contains(&function)
        || (function != FO_DELETE && to.is_null())
    {
        return error::INVALID_PARAMETER as i32;
    }
    // SAFETY: as above: `pFrom` holds names each ended by a NUL, the list by another.
    let sources = unsafe { super::processes::listed(from) };
    let destinations = match to.is_null() {
        true => &[][..],
        // SAFETY: as above, for `pTo`.
        false => unsafe { super::processes::listed(to) },
    };
    let sources = sources
        .split(|unit| *unit == 0)
        .filter(|name| !name.is_empty());
    let mut destinations = destinations
        .split(|unit| *unit == 0)
        .filter(|name| !name.is_empty());
    let many = sources.clone().count() > 1;
    let single = destinations.clone().next();
    let job = Operation { function, flags };
    for source in sources {
        let destination = match (function, flags & FOF_MULTIDESTFILES != 0) {
            (FO_DELETE, _) => None,
            (_, true) => match destinations.next() {
                Some(destination) => Some(destination),
                None => return error::INVALID_PARAMETER as i32,
            },
            (_, false) => single,
        };
        let many = many && flags & FOF_MULTIDESTFILES == 0;
        if let Err(code) = run_one(job, source, destination, many) {
            return code as i32;
        }
    }
    0
}

extern "efiapi" fn sh_empty_recycle_bin_w(_window: usize, _root: *const u16, flags: u32) -> i32 {
    if flags & SHERB_NOCONFIRMATION == 0 {
        return from_win32(error::NOT_SUPPORTED);
    }
    let (mut files, mut info) = ([0u8; PATH_CAPACITY], [0u8; PATH_CAPACITY]);
    let emptied = trash_folder(b"files", &mut files)
        .and_then(|length| remove_children(&files[..length], 0))
        .and_then(|()| trash_folder(b"info", &mut info))
        .and_then(|length| remove_children(&info[..length], 0));
    match emptied {
        Ok(()) => S_OK,
        Err(code) => from_win32(code),
    }
}

const EXECUTE_INFO_SIZE: u32 = 112;
const EXECUTE_MASK: usize = 4;
const EXECUTE_VERB: usize = 16;
const EXECUTE_FILE: usize = 24;
const EXECUTE_PARAMETERS: usize = 32;
const EXECUTE_DIRECTORY: usize = 40;
const EXECUTE_INSTANCE: usize = 56;
const EXECUTE_PROCESS: usize = 104;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
const SE_ERR_FNF: usize = 2;
const SE_ERR_NOASSOC: usize = 31;
const SUCCESS_INSTANCE: usize = 33;
const VERBS: [&str; 2] = ["open", "runas"];

unsafe fn field<T>(info: *mut u8, offset: usize) -> T {
    // SAFETY: the callers' contract: a `SHELLEXECUTEINFOW` holding a `T` at `offset`.
    unsafe { info.wrapping_add(offset).cast::<T>().read_unaligned() }
}

fn command_line_for(file: &[u16], parameters: &[u16]) -> Option<(*mut u16, usize)> {
    let length = file.len() + 3 + parameters.len();
    let block = heap::allocate(length * 2).cast::<u16>();
    if block.is_null() {
        return None;
    }
    // SAFETY: a fresh block of `length` units.
    let line = unsafe { core::slice::from_raw_parts_mut(block, length) };
    line[0] = u16::from(b'"');
    line[1..=file.len()].copy_from_slice(file);
    line[file.len() + 1] = u16::from(b'"');
    let mut used = file.len() + 2;
    if !parameters.is_empty() {
        line[used] = u16::from(b' ');
        line[used + 1..used + 1 + parameters.len()].copy_from_slice(parameters);
        used += 1 + parameters.len();
    }
    Some((block, used))
}

unsafe fn execute(info: *mut u8) -> Result<usize, (u32, usize)> {
    let string = |offset: usize| {
        // SAFETY: the callers' contract: the structure's string pointers are NUL-terminated
        // strings or null.
        let pointer: *const u16 = unsafe { field(info, offset) };
        match pointer.is_null() {
            true => None,
            // SAFETY: as above.
            false => Some(unsafe { super::file::wide_string(pointer) }),
        }
    };
    let file = string(EXECUTE_FILE).ok_or((error::INVALID_PARAMETER, SE_ERR_FNF))?;
    if let Some(verb) = string(EXECUTE_VERB)
        && !verb.is_empty()
        && !VERBS.iter().any(|known| {
            known.len() == verb.len()
                && known
                    .bytes()
                    .zip(verb)
                    .all(|(byte, unit)| u16::from(byte) == text::lower(*unit))
        })
    {
        return Err((error::NO_ASSOCIATION, SE_ERR_NOASSOC));
    }
    let mut program = [0u8; PATH_CAPACITY];
    let found = super::processes::find_program(file, &mut program);
    let runnable = found.is_some_and(|length| super::processes::is_program(&program[..length]));
    if !runnable {
        let mut path = [0u8; PATH_CAPACITY];
        let named = text::eggos_path(text::utf16_chars(file), &mut path);
        let is_url = file
            .windows(3)
            .any(|three| three == [u16::from(b':'), u16::from(b'/'), u16::from(b'/')]);
        return match (
            found,
            named.and_then(|length| exists(&path[..length])),
            is_url,
        ) {
            (Some(_), _, _) | (_, Some(_), _) | (_, _, true) => {
                Err((error::NO_ASSOCIATION, SE_ERR_NOASSOC))
            }
            _ => Err((error::FILE_NOT_FOUND, SE_ERR_FNF)),
        };
    }
    let parameters = string(EXECUTE_PARAMETERS).unwrap_or(&[]);
    let (block, used) =
        command_line_for(file, parameters).ok_or((error::NOT_ENOUGH_MEMORY, SE_ERR_FNF))?;
    // SAFETY: `command_line_for` filled `used` units of the block.
    let line = unsafe { core::slice::from_raw_parts(block, used) };
    let launch = super::processes::Launch {
        application: None,
        command_line: line,
        directory: string(EXECUTE_DIRECTORY),
        environment: super::processes::Environment::Inherited,
        streams: [0, 1, 2],
        own_group: false,
        inherit_handles: false,
        suspended: false,
    };
    let started = super::processes::launch(&launch);
    // SAFETY: the block is the one `command_line_for` made, no longer used.
    unsafe { heap::free(block.cast()) };
    started
        .map(|(handle, _)| handle)
        .map_err(|code| (code, SE_ERR_FNF))
}

unsafe extern "efiapi" fn shell_execute_ex_w(info: *mut u8) -> Bool {
    if info.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `ShellExecuteExW`'s contract: a `SHELLEXECUTEINFOW`, its size first.
    if unsafe { field::<u32>(info, 0) } != EXECUTE_INFO_SIZE {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: as above.
    let mask: u32 = unsafe { field(info, EXECUTE_MASK) };
    // SAFETY: as above.
    let outcome = unsafe { execute(info) };
    let instance = match outcome {
        Ok(_) => SUCCESS_INSTANCE,
        Err((_, instance)) => instance,
    };
    // SAFETY: as above: the structure is writable.
    unsafe {
        info.wrapping_add(EXECUTE_INSTANCE)
            .cast::<usize>()
            .write_unaligned(instance)
    };
    match outcome {
        Ok(handle) => {
            let kept = match mask & SEE_MASK_NOCLOSEPROCESS != 0 {
                true => handle,
                false => {
                    super::sync::close(handle);
                    0
                }
            };
            // SAFETY: as above.
            unsafe {
                info.wrapping_add(EXECUTE_PROCESS)
                    .cast::<usize>()
                    .write_unaligned(kept)
            };
            TRUE
        }
        Err((code, _)) => fail(code, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SHFileOperationW" => sh_file_operation_w,
    b"SHEmptyRecycleBinW" => sh_empty_recycle_bin_w,
    b"ShellExecuteExW" => shell_execute_ex_w,
];
