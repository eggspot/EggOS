//! Other programs (D190): `fork`, the `exec` family, `posix_spawn` and `posix_spawnp` with their
//! file actions and attributes, `wait`, `waitpid`, `wait3` and `wait4`, and `popen`, `pclose` and
//! `system`, which run EggOS's shell as `sh -c -- command`. What a path names is whatever the
//! kernel runs: another macOS program starts the layer again, a Linux program runs as it is.
//! A new child reports a failure to start the program through a close-on-exec pipe, so
//! `posix_spawn` returns the reason and no child is left behind.

use super::file::{duplicate_to, keep_across_exec, open_pipe};
use super::variadic::{RawVaList, VaList, variadic};
use super::{Exclusive, c_string, c_string_with_nul, errno_from, heap, process, set_errno};
use super::{signal, stdio, thread};
use crate::darwin::{self, EINVAL, ENOMEM, LinuxLayout};
use crate::darwin_signals;
use crate::format::Arguments;
use crate::linux;
use crate::spawn_actions::{self, Attributes, FileAction, Step, flag};
use core::ffi::{c_char, c_int, c_short, c_void};
use core::ptr;
use eggos_runtime::errno::Errno;

const LINUX_SIGCHLD: usize = 17;
const LINUX_SIGINT: usize = 2;
const LINUX_SIGQUIT: usize = 3;
const DARWIN_SIGINT_AND_SIGQUIT: u32 = 0b110;
const CLOSE_RANGE_CLOEXEC: usize = 4;
const UNCHANGED_ID: usize = u32::MAX as usize;
const EBADF: Errno = Errno(9);
const DARWIN_EBADF: c_int = 9;
const DARWIN_ECHILD: c_int = 10;
const DARWIN_EMFILE: c_int = 24;
const LIST_LIMIT: usize = 256;
const POPEN_LIMIT: usize = 64;
const SHELL: &core::ffi::CStr = c"/bin/eggsh";
const SHELL_NAME: &core::ffi::CStr = c"sh";

type Path = *mut c_char;

/// A `posix_spawn_file_actions_t`'s list, which the program holds a pointer to.
#[derive(Debug)]
pub struct Actions {
    list: *mut FileAction<Path>,
    count: usize,
    capacity: usize,
}

static OPENED: Exclusive<[(c_int, c_int); POPEN_LIMIT]> = Exclusive::new([(0, 0); POPEN_LIMIT]);

fn memoryless_call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: the calls made through this (`setsid`, `setpgid`, the ids, `fchdir` and
    // `close_range`) touch no memory.
    unsafe { linux::syscall(number, arguments) }
}

fn fork_process() -> Result<usize, Errno> {
    // SAFETY: `clone` with only `SIGCHLD` copies the whole process, so the child goes on from
    // here with its own copy of every page, as after `fork`.
    let forked = unsafe { linux::syscall(linux::number::CLONE, [LINUX_SIGCHLD, 0, 0, 0, 0, 0]) }?;
    if forked == 0 {
        thread::adopt_after_fork();
    }
    Ok(forked)
}

pub extern "C" fn fork() -> c_int {
    fork_process().map_or_else(|errno| errno_from(errno, -1), |pid| pid as c_int)
}

fn environment() -> *const *const c_char {
    // SAFETY: `ENVIRON` is only replaced whole, by `setenv` and its kin.
    unsafe { process::ENVIRON }.cast_const().cast()
}

fn search_path<'a>() -> &'a [u8] {
    // SAFETY: `getenv` reads the NUL-terminated name.
    let value = unsafe { process::getenv(c"PATH".as_ptr()) };
    if value.is_null() {
        spawn_actions::DEFAULT_SEARCH_PATH
    } else {
        // SAFETY: an environment value is NUL-terminated and stays while `PATH` is unchanged.
        unsafe { c_string(value) }
    }
}

/// Runs the program at the NUL-terminated `path`; returns only why it could not.
fn execute(
    path: &[u8],
    arguments: *const *const c_char,
    environment: *const *const c_char,
) -> Errno {
    // SAFETY: `execve` reads the NUL-terminated path and the program's null-ended argument and
    // environment lists; it returns only when it fails.
    unsafe {
        linux::syscall(
            linux::number::EXECVE,
            [
                path.as_ptr() as usize,
                arguments as usize,
                environment as usize,
                0,
                0,
                0,
            ],
        )
    }
    .err()
    .unwrap_or(Errno::EINVAL)
}

/// Runs `file` as `execvp` does: as it is if it names a directory, else from each directory of
/// `search` in turn. Returns only why it could not.
///
/// # Safety
/// `file` must be a NUL-terminated string.
unsafe fn execute_searching(
    file: *const c_char,
    search: &[u8],
    arguments: *const *const c_char,
    environment: *const *const c_char,
) -> Errno {
    // SAFETY: the caller's promise.
    let name = unsafe { c_string(file) };
    if name.is_empty() {
        return Errno::ENOENT;
    }
    if spawn_actions::is_path(name) {
        // SAFETY: as above.
        return execute(unsafe { c_string_with_nul(file) }, arguments, environment);
    }
    let mut buffer = [0; spawn_actions::PATH_MAX];
    let mut failure = Errno::ENOENT;
    let mut denied = false;
    for directory in search.split(|byte| *byte == b':') {
        let Ok(path) = spawn_actions::candidate(directory, name, &mut buffer) else {
            failure = Errno::ENAMETOOLONG;
            continue;
        };
        match execute(path, arguments, environment) {
            Errno::EACCES => denied = true,
            Errno::ENOENT | Errno::ENOTDIR | Errno::ENAMETOOLONG => {}
            other => return other,
        }
    }
    if denied { Errno::EACCES } else { failure }
}

/// # Safety
/// `path` must be a NUL-terminated string, and `arguments` and `environment` null-ended lists
/// of them.
pub unsafe extern "C" fn execve(
    path: *const c_char,
    arguments: *const *const c_char,
    environment: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    let path = unsafe { c_string_with_nul(path) };
    errno_from(execute(path, arguments, environment), -1)
}

/// # Safety
/// As [`execve`].
pub unsafe extern "C" fn execv(path: *const c_char, arguments: *const *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { execve(path, arguments, environment()) }
}

/// # Safety
/// As [`execve`].
pub unsafe extern "C" fn execvp(file: *const c_char, arguments: *const *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    let failure = unsafe { execute_searching(file, search_path(), arguments, environment()) };
    errno_from(failure, -1)
}

/// Darwin's `execvP`: `execvp` with the directories to search given.
///
/// # Safety
/// As [`execve`]; `search` must be a NUL-terminated string.
pub unsafe extern "C" fn execv_searching(
    file: *const c_char,
    search: *const c_char,
    arguments: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    let search = unsafe { c_string(search) };
    // SAFETY: the caller's promise.
    let failure = unsafe { execute_searching(file, search, arguments, environment()) };
    errno_from(failure, -1)
}

/// Reads an `execl` list, `first` then the variadic arguments up to the null one, into `list`,
/// and returns the arguments that follow it.
///
/// # Safety
/// `arguments` must be the variadic arguments of an `execl` call, which end with a null
/// pointer.
unsafe fn read_list(
    first: *const c_char,
    arguments: RawVaList,
    list: &mut [*const c_char; LIST_LIMIT + 1],
) -> Result<VaList, Errno> {
    // SAFETY: the caller's promise.
    let mut rest = unsafe { VaList::new(arguments) };
    list[0] = first;
    let mut count = 1;
    let mut last = first;
    while !last.is_null() {
        if count > LIST_LIMIT {
            return Err(Errno::E2BIG);
        }
        last = ptr::with_exposed_provenance(rest.integer() as usize);
        list[count] = last;
        count += 1;
    }
    Ok(rest)
}

unsafe extern "C" fn execl_with(
    path: *const c_char,
    first: *const c_char,
    arguments: RawVaList,
) -> c_int {
    let mut list = [ptr::null(); LIST_LIMIT + 1];
    // SAFETY: the program passes `execl`'s list, which ends with a null pointer.
    if let Err(errno) = unsafe { read_list(first, arguments, &mut list) } {
        return errno_from(errno, -1);
    }
    // SAFETY: the program passes a NUL-terminated path.
    unsafe { execve(path, list.as_ptr(), environment()) }
}

variadic!(eggos_macos_execl => execl_with, 2, "x2", "rdx");
pub use eggos_macos_execl as execl;

unsafe extern "C" fn execle_with(
    path: *const c_char,
    first: *const c_char,
    arguments: RawVaList,
) -> c_int {
    let mut list = [ptr::null(); LIST_LIMIT + 1];
    // SAFETY: the program passes `execle`'s list, which ends with a null pointer, then the
    // environment.
    let mut rest = match unsafe { read_list(first, arguments, &mut list) } {
        Ok(rest) => rest,
        Err(errno) => return errno_from(errno, -1),
    };
    let environment = ptr::with_exposed_provenance(rest.integer() as usize);
    // SAFETY: the program passes a NUL-terminated path.
    unsafe { execve(path, list.as_ptr(), environment) }
}

variadic!(eggos_macos_execle => execle_with, 2, "x2", "rdx");
pub use eggos_macos_execle as execle;

unsafe extern "C" fn execlp_with(
    file: *const c_char,
    first: *const c_char,
    arguments: RawVaList,
) -> c_int {
    let mut list = [ptr::null(); LIST_LIMIT + 1];
    // SAFETY: the program passes `execlp`'s list, which ends with a null pointer.
    if let Err(errno) = unsafe { read_list(first, arguments, &mut list) } {
        return errno_from(errno, -1);
    }
    // SAFETY: the program passes a NUL-terminated file name.
    unsafe { execvp(file, list.as_ptr()) }
}

variadic!(eggos_macos_execlp => execlp_with, 2, "x2", "rdx");
pub use eggos_macos_execlp as execlp;

fn take(step: Step<Path>) -> Result<(), Errno> {
    let done = |result: Result<usize, Errno>| result.map(|_| ());
    match step {
        Step::CloseAllOnExec => done(memoryless_call(
            linux::number::CLOSE_RANGE,
            [0, UNCHANGED_ID, CLOSE_RANGE_CLOEXEC, 0, 0, 0],
        )),
        Step::NewSession => done(memoryless_call(linux::number::SETSID, [0; 6])),
        Step::SetProcessGroup(group) => done(memoryless_call(
            linux::number::SETPGID,
            [0, group as usize, 0, 0, 0, 0],
        )),
        Step::ResetIds => reset_ids(),
        Step::DefaultSignals(set) => signal::set_default(set),
        Step::SetMask(set) => {
            signal::swap_linux_mask(darwin_signals::linux_signal_set(set)).map(|_| ())
        }
        Step::Open {
            fd,
            path,
            flags,
            mode,
        } => open_as(fd, path, flags, mode),
        Step::Close(fd) => match linux::close(fd as usize) {
            Err(EBADF) => Ok(()),
            closed => done(closed),
        },
        Step::Duplicate { from, to } => done(duplicate_to(from, to)),
        Step::KeepAcrossExec(fd) => done(keep_across_exec(fd)),
        Step::ChangeDirectory(path) => {
            // SAFETY: `chdir` reads the action's own NUL-terminated copy of the path.
            done(unsafe { linux::syscall(linux::number::CHDIR, [path as usize, 0, 0, 0, 0, 0]) })
        }
        Step::ChangeDirectoryTo(fd) => done(memoryless_call(
            linux::number::FCHDIR,
            [fd as usize, 0, 0, 0, 0, 0],
        )),
    }
}

fn reset_ids() -> Result<(), Errno> {
    let group = memoryless_call(linux::number::GETGID, [0; 6])?;
    memoryless_call(
        linux::number::SETRESGID,
        [UNCHANGED_ID, group, UNCHANGED_ID, 0, 0, 0],
    )?;
    let user = memoryless_call(linux::number::GETUID, [0; 6])?;
    memoryless_call(
        linux::number::SETRESUID,
        [UNCHANGED_ID, user, UNCHANGED_ID, 0, 0, 0],
    )?;
    Ok(())
}

fn open_as(fd: c_int, path: Path, flags: u32, mode: u32) -> Result<(), Errno> {
    // SAFETY: `path` is the action's own NUL-terminated copy.
    let path = unsafe { c_string_with_nul(path) };
    let opened = linux::open(path, flags as usize, mode as usize)?;
    if opened != fd as usize {
        let moved = duplicate_to(opened as c_int, fd);
        let _ = linux::close(opened);
        moved?;
    }
    Ok(())
}

/// What a program started by `posix_spawn` runs: `file`, searched for with `search`.
#[derive(Debug, Clone, Copy)]
struct Program {
    file: *const c_char,
    search: bool,
    arguments: *const *const c_char,
    environment: *const *const c_char,
}

impl Program {
    /// Takes the steps of `attributes` and `actions`, then becomes the program; returns only
    /// why it could not.
    ///
    /// # Safety
    /// `file` must be a NUL-terminated string, and `arguments` and `environment` null-ended
    /// lists of them.
    unsafe fn become_it(&self, attributes: Attributes, actions: &[FileAction<Path>]) -> Errno {
        for step in spawn_actions::steps(attributes, actions) {
            if let Err(errno) = take(step) {
                return errno;
            }
        }
        if self.search {
            // SAFETY: the caller's promise.
            unsafe { execute_searching(self.file, search_path(), self.arguments, self.environment) }
        } else {
            // SAFETY: the caller's promise.
            let path = unsafe { c_string_with_nul(self.file) };
            execute(path, self.arguments, self.environment)
        }
    }

    /// Starts the program in a new child, which takes the steps first; the child's pid, or
    /// why it could not start (then no child is left).
    ///
    /// # Safety
    /// As [`Program::become_it`].
    unsafe fn start(
        &self,
        attributes: Attributes,
        actions: &[FileAction<Path>],
    ) -> Result<c_int, Errno> {
        let mut report = [0; 2];
        open_pipe(&mut report, linux::O_CLOEXEC)?;
        let [report_read, report_write] = report;
        let saved_mask = match signal::block_all() {
            Ok(mask) => mask,
            Err(errno) => {
                let _ = linux::close(report_read as usize);
                let _ = linux::close(report_write as usize);
                return Err(errno);
            }
        };
        let forked = fork_process();
        if matches!(forked, Ok(0)) {
            signal::reset_caught();
            let failure = match signal::swap_linux_mask(saved_mask) {
                // SAFETY: the caller's promise.
                Ok(_) => unsafe { self.become_it(attributes, actions) },
                Err(errno) => errno,
            };
            let _ = linux::write_all(report_write as usize, &failure.0.to_le_bytes());
            linux::exit(127);
        }
        let _ = signal::swap_linux_mask(saved_mask);
        let _ = linux::close(report_write as usize);
        let pid = match forked {
            Ok(pid) => pid as c_int,
            Err(errno) => {
                let _ = linux::close(report_read as usize);
                return Err(errno);
            }
        };
        let mut failure = [0; 2];
        let reported = read_report(report_read, &mut failure);
        let _ = linux::close(report_read as usize);
        if reported == failure.len() {
            let _ = wait_for(pid);
            return Err(Errno(u16::from_le_bytes(failure)));
        }
        Ok(pid)
    }
}

fn read_report(fd: c_int, buffer: &mut [u8; 2]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match linux::read(fd as usize, &mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(Errno::EINTR) => {}
            Err(_) => break,
        }
    }
    filled
}

fn wait_raw(
    pid: c_int,
    status: *mut c_int,
    options: usize,
    usage: *mut c_void,
) -> Result<usize, Errno> {
    // SAFETY: `wait4` writes a status and a `struct rusage` (the same on Darwin and Linux)
    // into the given memory, if any.
    unsafe {
        linux::syscall(
            linux::number::WAIT4,
            [pid as usize, status as usize, options, usage as usize, 0, 0],
        )
    }
}

/// Waits for the child `pid` to end and returns its Darwin wait status, or -1 with `errno`.
fn wait_for(pid: c_int) -> c_int {
    let mut status = 0;
    loop {
        match wait_raw(pid, &mut status, 0, ptr::null_mut()) {
            Ok(_) => return darwin_signals::darwin_wait_status(status),
            Err(Errno::EINTR) => {}
            Err(errno) => return errno_from(errno, -1),
        }
    }
}

/// # Safety
/// `status` must be null or writable, and `usage` null or writable for a `struct rusage`.
pub unsafe extern "C" fn wait4(
    pid: c_int,
    status: *mut c_int,
    options: c_int,
    usage: *mut c_void,
) -> c_int {
    let Some(linux_options) = darwin_signals::linux_wait_options(options) else {
        set_errno(EINVAL);
        return -1;
    };
    let mut linux_status = 0;
    match wait_raw(pid, &mut linux_status, linux_options, usage) {
        Ok(child) => {
            if child != 0 && !status.is_null() {
                // SAFETY: the caller's promise.
                unsafe { status.write(darwin_signals::darwin_wait_status(linux_status)) };
            }
            child as c_int
        }
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `status` must be null or writable.
pub unsafe extern "C" fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { wait4(pid, status, options, ptr::null_mut()) }
}

/// # Safety
/// As [`waitpid`].
pub unsafe extern "C" fn wait(status: *mut c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { wait4(-1, status, 0, ptr::null_mut()) }
}

/// # Safety
/// As [`wait4`].
pub unsafe extern "C" fn wait3(status: *mut c_int, options: c_int, usage: *mut c_void) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { wait4(-1, status, options, usage) }
}

/// # Safety
/// `path` must be a NUL-terminated string.
unsafe fn copy_path(path: *const c_char) -> Path {
    // SAFETY: the caller's promise.
    let bytes = unsafe { c_string_with_nul(path) };
    let copy = heap::allocate(bytes.len(), false);
    if !copy.is_null() {
        // SAFETY: the new block holds `bytes.len()` bytes and overlaps nothing.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), copy, bytes.len()) };
    }
    copy.cast()
}

fn release_path(action: &FileAction<Path>) {
    if let FileAction::Open { path, .. } | FileAction::ChangeDirectory { path } = action {
        heap::release(path.cast());
    }
}

/// # Safety
/// `actions` must be null or point to a `posix_spawn_file_actions_t`.
unsafe fn actions_of<'a>(actions: *const *mut Actions) -> Option<&'a mut Actions> {
    if actions.is_null() {
        return None;
    }
    // SAFETY: the caller's promise; `init` set the pointer to the layer's list, or it is null.
    unsafe { actions.read().as_mut() }
}

/// # Safety
/// `actions` must be writable.
pub unsafe extern "C" fn file_actions_init(actions: *mut *mut Actions) -> c_int {
    let list = heap::allocate(size_of::<Actions>(), false).cast::<Actions>();
    if list.is_null() {
        return ENOMEM;
    }
    // SAFETY: the new block holds an `Actions`.
    unsafe {
        list.write(Actions {
            list: ptr::null_mut(),
            count: 0,
            capacity: 0,
        })
    };
    // SAFETY: the caller's promise.
    unsafe { actions.write(list) };
    0
}

/// # Safety
/// `actions` must point to a `posix_spawn_file_actions_t` that `init` set up.
pub unsafe extern "C" fn file_actions_destroy(actions: *mut *mut Actions) -> c_int {
    // SAFETY: the caller's promise.
    let Some(list) = (unsafe { actions_of(actions) }) else {
        return EINVAL;
    };
    for action in list.entries() {
        release_path(action);
    }
    heap::release(list.list.cast());
    heap::release(ptr::from_mut(list).cast());
    // SAFETY: the caller's promise.
    unsafe { actions.write(ptr::null_mut()) };
    0
}

impl Actions {
    fn entries(&self) -> &[FileAction<Path>] {
        if self.list.is_null() {
            return &[];
        }
        // SAFETY: `list` holds `count` actions that `push` wrote.
        unsafe { core::slice::from_raw_parts(self.list, self.count) }
    }

    fn push(&mut self, action: FileAction<Path>) -> c_int {
        if self.count == self.capacity {
            let capacity = (self.capacity * 2).max(4);
            let size = capacity * size_of::<FileAction<Path>>();
            let grown = heap::reallocate(self.list.cast(), size);
            if grown.is_null() {
                release_path(&action);
                return ENOMEM;
            }
            self.list = grown.cast();
            self.capacity = capacity;
        }
        // SAFETY: the list has room for one more action.
        unsafe { self.list.add(self.count).write(action) };
        self.count += 1;
        0
    }
}

/// # Safety
/// `actions` must point to a `posix_spawn_file_actions_t`; paths in `action` must be the
/// layer's own copies.
unsafe fn add(actions: *const *mut Actions, action: FileAction<Path>) -> c_int {
    // SAFETY: the caller's promise.
    match unsafe { actions_of(actions) } {
        Some(list) => list.push(action),
        None => {
            release_path(&action);
            EINVAL
        }
    }
}

/// # Safety
/// `actions` must point to a `posix_spawn_file_actions_t`.
unsafe fn add_descriptor_action(
    actions: *const *mut Actions,
    fds: &[c_int],
    action: FileAction<Path>,
) -> c_int {
    if !fds.iter().all(|fd| spawn_actions::valid_descriptor(*fd)) {
        return DARWIN_EBADF;
    }
    // SAFETY: the caller's promise; the action has no path.
    unsafe { add(actions, action) }
}

/// # Safety
/// `actions` must point to a `posix_spawn_file_actions_t` and `path` be a NUL-terminated
/// string.
pub unsafe extern "C" fn file_actions_add_open(
    actions: *mut *mut Actions,
    fd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: u16,
) -> c_int {
    if !spawn_actions::valid_descriptor(fd) {
        return DARWIN_EBADF;
    }
    let Some(linux_flags) = darwin::linux_open_flags(flags as u32, LinuxLayout::HOST) else {
        return EINVAL;
    };
    // SAFETY: the caller's promise.
    let path = unsafe { copy_path(path) };
    if path.is_null() {
        return ENOMEM;
    }
    let action = FileAction::Open {
        fd,
        path,
        flags: linux_flags,
        mode: u32::from(mode),
    };
    // SAFETY: the caller's promise; the path is the layer's copy.
    unsafe { add(actions, action) }
}

/// # Safety
/// `actions` must point to a `posix_spawn_file_actions_t`.
pub unsafe extern "C" fn file_actions_add_close(actions: *mut *mut Actions, fd: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { add_descriptor_action(actions, &[fd], FileAction::Close { fd }) }
}

/// # Safety
/// As [`file_actions_add_close`].
pub unsafe extern "C" fn file_actions_add_dup2(
    actions: *mut *mut Actions,
    from: c_int,
    to: c_int,
) -> c_int {
    let action = FileAction::Duplicate { from, to };
    // SAFETY: the caller's promise.
    unsafe { add_descriptor_action(actions, &[from, to], action) }
}

/// # Safety
/// As [`file_actions_add_close`].
pub unsafe extern "C" fn file_actions_add_inherit(actions: *mut *mut Actions, fd: c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { add_descriptor_action(actions, &[fd], FileAction::Inherit { fd }) }
}

/// # Safety
/// As [`file_actions_add_close`].
pub unsafe extern "C" fn file_actions_add_fchdir(actions: *mut *mut Actions, fd: c_int) -> c_int {
    let action = FileAction::ChangeDirectoryTo { fd };
    // SAFETY: the caller's promise.
    unsafe { add_descriptor_action(actions, &[fd], action) }
}

/// # Safety
/// As [`file_actions_add_open`].
pub unsafe extern "C" fn file_actions_add_chdir(
    actions: *mut *mut Actions,
    path: *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    let path = unsafe { copy_path(path) };
    if path.is_null() {
        return ENOMEM;
    }
    // SAFETY: the caller's promise; the path is the layer's copy.
    unsafe { add(actions, FileAction::ChangeDirectory { path }) }
}

/// # Safety
/// `attributes` must be null or point to a `posix_spawnattr_t`.
unsafe fn attributes_of<'a>(attributes: *const *mut Attributes) -> Option<&'a mut Attributes> {
    if attributes.is_null() {
        return None;
    }
    // SAFETY: the caller's promise; `init` set the pointer to the layer's settings, or it is
    // null.
    unsafe { attributes.read().as_mut() }
}

/// # Safety
/// `attributes` must be writable.
pub unsafe extern "C" fn attributes_init(attributes: *mut *mut Attributes) -> c_int {
    let settings = heap::allocate(size_of::<Attributes>(), false).cast::<Attributes>();
    if settings.is_null() {
        return ENOMEM;
    }
    // SAFETY: the new block holds an `Attributes`.
    unsafe { settings.write(Attributes::default()) };
    // SAFETY: the caller's promise.
    unsafe { attributes.write(settings) };
    0
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` that `init` set up.
pub unsafe extern "C" fn attributes_destroy(attributes: *mut *mut Attributes) -> c_int {
    // SAFETY: the caller's promise.
    let Some(settings) = (unsafe { attributes_of(attributes) }) else {
        return EINVAL;
    };
    heap::release(ptr::from_mut(settings).cast());
    // SAFETY: the caller's promise.
    unsafe { attributes.write(ptr::null_mut()) };
    0
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t`.
unsafe fn change_attributes(
    attributes: *const *mut Attributes,
    change: impl FnOnce(&mut Attributes),
) -> c_int {
    // SAFETY: the caller's promise.
    match unsafe { attributes_of(attributes) } {
        Some(settings) => {
            change(settings);
            0
        }
        None => EINVAL,
    }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `out` be writable.
unsafe fn read_attribute<T>(
    attributes: *const *mut Attributes,
    out: *mut T,
    read: impl FnOnce(&Attributes) -> T,
) -> c_int {
    // SAFETY: the caller's promise.
    match unsafe { attributes_of(attributes) } {
        Some(settings) => {
            // SAFETY: the caller's promise.
            unsafe { out.write(read(settings)) };
            0
        }
        None => EINVAL,
    }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t`.
pub unsafe extern "C" fn attributes_set_flags(
    attributes: *mut *mut Attributes,
    flags: c_short,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { change_attributes(attributes, |settings| settings.flags = flags) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `flags` be writable.
pub unsafe extern "C" fn attributes_get_flags(
    attributes: *const *mut Attributes,
    flags: *mut c_short,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { read_attribute(attributes, flags, |settings| settings.flags) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t`.
pub unsafe extern "C" fn attributes_set_process_group(
    attributes: *mut *mut Attributes,
    group: c_int,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { change_attributes(attributes, |settings| settings.process_group = group) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `group` be writable.
pub unsafe extern "C" fn attributes_get_process_group(
    attributes: *const *mut Attributes,
    group: *mut c_int,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { read_attribute(attributes, group, |settings| settings.process_group) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `set` be readable.
pub unsafe extern "C" fn attributes_set_mask(
    attributes: *mut *mut Attributes,
    set: *const u32,
) -> c_int {
    // SAFETY: the caller's promise.
    let set = unsafe { set.read() };
    // SAFETY: the caller's promise.
    unsafe { change_attributes(attributes, |settings| settings.mask = set) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `set` be writable.
pub unsafe extern "C" fn attributes_get_mask(
    attributes: *const *mut Attributes,
    set: *mut u32,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { read_attribute(attributes, set, |settings| settings.mask) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `set` be readable.
pub unsafe extern "C" fn attributes_set_default_signals(
    attributes: *mut *mut Attributes,
    set: *const u32,
) -> c_int {
    // SAFETY: the caller's promise.
    let set = unsafe { set.read() };
    // SAFETY: the caller's promise.
    unsafe { change_attributes(attributes, |settings| settings.default_signals = set) }
}

/// # Safety
/// `attributes` must point to a `posix_spawnattr_t` and `set` be writable.
pub unsafe extern "C" fn attributes_get_default_signals(
    attributes: *const *mut Attributes,
    set: *mut u32,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { read_attribute(attributes, set, |settings| settings.default_signals) }
}

/// `posix_spawn` and `posix_spawnp`: the child's pid into `pid`, and 0 or Darwin's `errno`.
///
/// # Safety
/// The pointers must be as `posix_spawn` takes them.
unsafe fn spawn(
    pid: *mut c_int,
    program: Program,
    actions: *const *mut Actions,
    attributes: *const *mut Attributes,
) -> c_int {
    // SAFETY: the caller's promise.
    let settings = unsafe { attributes_of(attributes) }.map_or_else(Attributes::default, |s| *s);
    if let Err(errno) = settings.check() {
        return errno;
    }
    // SAFETY: the caller's promise.
    let list = unsafe { actions_of(actions) }.map_or(&[][..], |list| list.entries());
    let program = Program {
        environment: if program.environment.is_null() {
            environment()
        } else {
            program.environment
        },
        ..program
    };
    if settings.replaces_caller() {
        // SAFETY: the caller's promise.
        let failure = unsafe { program.become_it(settings, list) };
        return darwin::darwin_errno(failure.0);
    }
    // SAFETY: the caller's promise.
    match unsafe { program.start(settings, list) } {
        Ok(child) => {
            if !pid.is_null() {
                // SAFETY: the caller's promise.
                unsafe { pid.write(child) };
            }
            0
        }
        Err(errno) => darwin::darwin_errno(errno.0),
    }
}

/// # Safety
/// As `posix_spawn` takes them: `pid` null or writable, `path` a NUL-terminated string,
/// `actions` and `attributes` null or set up by their `init`, `arguments` and `environment`
/// null-ended lists of strings (a null `environment` is the program's own).
pub unsafe extern "C" fn posix_spawn(
    pid: *mut c_int,
    path: *const c_char,
    actions: *const *mut Actions,
    attributes: *const *mut Attributes,
    arguments: *const *const c_char,
    environment: *const *const c_char,
) -> c_int {
    let program = Program {
        file: path,
        search: false,
        arguments,
        environment,
    };
    // SAFETY: the caller's promise.
    unsafe { spawn(pid, program, actions, attributes) }
}

/// # Safety
/// As [`posix_spawn`], with `file` searched for in `PATH` unless it names a directory.
pub unsafe extern "C" fn posix_spawnp(
    pid: *mut c_int,
    file: *const c_char,
    actions: *const *mut Actions,
    attributes: *const *mut Attributes,
    arguments: *const *const c_char,
    environment: *const *const c_char,
) -> c_int {
    let program = Program {
        file,
        search: true,
        arguments,
        environment,
    };
    // SAFETY: the caller's promise.
    unsafe { spawn(pid, program, actions, attributes) }
}

/// Starts EggOS's shell on `command` in a new child that takes `attributes` and `actions`
/// first.
///
/// # Safety
/// `command` must be a NUL-terminated string.
unsafe fn start_shell(
    command: *const c_char,
    attributes: Attributes,
    actions: &[FileAction<Path>],
) -> Result<c_int, Errno> {
    let arguments = [
        SHELL_NAME.as_ptr(),
        c"-c".as_ptr(),
        c"--".as_ptr(),
        command,
        ptr::null(),
    ];
    let program = Program {
        file: SHELL.as_ptr(),
        search: false,
        arguments: arguments.as_ptr(),
        environment: environment(),
    };
    // SAFETY: the shell's path and the arguments are NUL-terminated, the lists null-ended.
    unsafe { program.start(attributes, actions) }
}

/// # Safety
/// `command` and `mode` must be NUL-terminated strings.
pub unsafe extern "C" fn popen(command: *const c_char, mode: *const c_char) -> *mut stdio::Stream {
    // SAFETY: the caller's promise.
    let reading = match unsafe { c_string(mode) } {
        [b'r', rest @ ..] if !rest.contains(&b'+') => true,
        [b'w', rest @ ..] if !rest.contains(&b'+') => false,
        _ => {
            set_errno(EINVAL);
            return ptr::null_mut();
        }
    };
    let mut ends = [0; 2];
    if let Err(errno) = open_pipe(&mut ends, linux::O_CLOEXEC) {
        return errno_from(errno, ptr::null_mut());
    }
    let [read_end, write_end] = ends;
    let (ours, theirs, standard) = if reading {
        (read_end, write_end, 1)
    } else {
        (write_end, read_end, 0)
    };
    let actions = [FileAction::Duplicate {
        from: theirs,
        to: standard,
    }];
    // SAFETY: the caller's promise.
    let started = unsafe { start_shell(command, Attributes::default(), &actions) };
    let _ = linux::close(theirs as usize);
    let child = match started {
        Ok(child) => child,
        Err(errno) => {
            let _ = linux::close(ours as usize);
            return errno_from(errno, ptr::null_mut());
        }
    };
    let stream_mode = if reading { c"r" } else { c"w" };
    // SAFETY: the mode is a NUL-terminated string.
    let stream = unsafe { stdio::fdopen(ours, stream_mode.as_ptr()) };
    let recorded = !stream.is_null()
        && OPENED.with(
            |opened| match opened.iter_mut().find(|(_, pid)| *pid == 0) {
                Some(slot) => {
                    *slot = (ours, child);
                    true
                }
                None => false,
            },
        );
    if !recorded {
        if stream.is_null() {
            let _ = linux::close(ours as usize);
        } else {
            // SAFETY: `fdopen` just made the stream.
            unsafe { stdio::fclose(stream) };
        }
        let _ = wait_for(child);
        set_errno(DARWIN_EMFILE);
        return ptr::null_mut();
    }
    stream
}

/// # Safety
/// `stream` must be a stream `popen` returned.
pub unsafe extern "C" fn pclose(stream: *mut stdio::Stream) -> c_int {
    // SAFETY: the caller's promise.
    let fd = unsafe { stdio::fileno(stream) };
    let child = OPENED.with(|opened| {
        let slot = opened
            .iter_mut()
            .find(|(opened_fd, pid)| *pid != 0 && *opened_fd == fd)?;
        let child = slot.1;
        *slot = (0, 0);
        Some(child)
    });
    let Some(child) = child else {
        set_errno(DARWIN_ECHILD);
        return -1;
    };
    // SAFETY: the caller's promise.
    unsafe { stdio::fclose(stream) };
    wait_for(child)
}

/// # Safety
/// `command` must be null or a NUL-terminated string.
pub unsafe extern "C" fn system(command: *const c_char) -> c_int {
    if command.is_null() {
        return 1;
    }
    let saved_mask = signal::block_linux(1 << (LINUX_SIGCHLD - 1)).ok();
    let attributes = Attributes {
        flags: flag::SETSIGDEF
            | if saved_mask.is_some() {
                flag::SETSIGMASK
            } else {
                0
            },
        default_signals: DARWIN_SIGINT_AND_SIGQUIT,
        mask: saved_mask.map_or(0, darwin_signals::darwin_signal_set),
        process_group: 0,
    };
    let status = signal::with_ignored(&[LINUX_SIGINT, LINUX_SIGQUIT], || {
        // SAFETY: the caller's promise.
        match unsafe { start_shell(command, attributes, &[]) } {
            Ok(child) => wait_for(child),
            Err(errno) => errno_from(errno, -1),
        }
    });
    if let Some(mask) = saved_mask {
        let _ = signal::swap_linux_mask(mask);
    }
    status
}
