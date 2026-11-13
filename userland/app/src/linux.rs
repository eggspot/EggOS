//! The installer's system calls: [`Files`] over Linux's file calls, the machine's architecture,
//! the desktop's requests (`/dev/window`'s `APPS_CHANGED` and `OPEN_APP`, docs/windows.md),
//! the standard output and error, and starting the installer's window.

use crate::architecture::Architecture;
use crate::files::{Files, Handle, Kind, PATH_CAPACITY, Path, Space, Status};
use core::ffi::CStr;
use eggos_app_manifest::translation::LAYER_OPTION;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

const AT_FDCWD: usize = -100isize as usize;
const WRITE_ACCESS: usize = 2;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const AT_REMOVEDIR: usize = 0x200;
const O_RDONLY: usize = 0;
const O_WRONLY: usize = 0o1;
const O_RDWR: usize = 0o2;
const O_CREAT: usize = 0o100;
const O_EXCL: usize = 0o200;
const O_DIRECTORY: usize = 0o200_000;
const O_CLOEXEC: usize = 0o2_000_000;
const DIRECTORY_MODE: usize = 0o755;
const STATX_BASIC_STATS: usize = 0x7ff;
const STATX_SIZE: usize = 256;
const STATX_MODE: usize = 28;
const STATX_FILE_SIZE: usize = 40;
const STATX_MODIFIED: usize = 112;
const STATX_DEVICE_MAJOR: usize = 136;
const STATX_DEVICE_MINOR: usize = 140;
const STATFS_SIZE: usize = 120;
const STATFS_BLOCK_SIZE: usize = 8;
const STATFS_BLOCKS: usize = 16;
const STATFS_AVAILABLE: usize = 32;
const STATFS_FRAGMENT_SIZE: usize = 72;
const STATFS_FLAGS: usize = 80;
const STATFS_READ_ONLY: u64 = 0x1;
const FILE_TYPE: u32 = 0o170_000;
const REGULAR: u32 = 0o100_000;
const DIRECTORY: u32 = 0o040_000;
const SYMBOLIC_LINK: u32 = 0o120_000;
const UTSNAME_FIELD: usize = 65;
const UTSNAME_MACHINE: usize = 4;
const DIRENT_LENGTH: usize = 16;
const DIRENT_NAME: usize = 19;
const DIRECTORY_BUFFER: usize = 2048;
const WINDOW_DEVICE: &CStr = c"/dev/window";
const APPS_CHANGED: usize = 0x0000_770f;
const OPEN_APP: usize = 0x4044_7710;
const OPEN_APP_SIZE: usize = 68;
const GET_LAUNCH: usize = 0xc108_772b;
const GET_LAUNCH_SIZE: usize = 264;
const LAUNCH_START: u32 = 1;
const LAUNCH_TYPE: u32 = 2;
const LAUNCH_COMMAND: usize = 8;
const AT_FDCWD: usize = -100isize as usize;
const STANDARD_INPUT_LINK: &CStr = c"/proc/self/fd/0";
const CONSOLE: &[u8] = b"/dev/console";
const SHELL: &[u8] = b"/bin/eggsh";
const BAD_DESCRIPTOR: Errno = Errno(9);
const MAX_ARGUMENTS: usize = 16;
const SIGCHLD: usize = 17;
const EXEC_FAILED: i32 = 127;

/// The standard output.
pub const STANDARD_OUTPUT: usize = 1;
/// The standard error.
pub const STANDARD_ERROR: usize = 2;
/// The installer's window: the toolkit program's name for it (D134).
pub const WINDOW_PROGRAM: &CStr = c"/usr/libexec/app-window";

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes only buffers it owns and borrows for the call, sized as the
    // call reads or writes them, and no call here maps, unmaps or changes the program's memory.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

struct CPath {
    bytes: [u8; PATH_CAPACITY + 1],
}

impl CPath {
    fn new(path: &[u8]) -> Result<Self, Errno> {
        if path.len() > PATH_CAPACITY || path.contains(&0) {
            return Err(Errno::ENAMETOOLONG);
        }
        let mut bytes = [0u8; PATH_CAPACITY + 1];
        bytes[..path.len()].copy_from_slice(path);
        Ok(Self { bytes })
    }

    fn pointer(&self) -> usize {
        self.bytes.as_ptr() as usize
    }
}

/// Writes all of `bytes` to `descriptor`, ignoring errors.
pub fn write_all(descriptor: usize, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        match call(
            number::WRITE,
            [descriptor, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
        ) {
            Ok(0) => return,
            Ok(written) => bytes = &bytes[written.min(bytes.len())..],
            Err(Errno::EINTR) => {}
            Err(_) => return,
        }
    }
}

/// The machine's architecture, as `uname -m` says it.
pub fn machine() -> Architecture {
    let mut name = [0u8; 6 * UTSNAME_FIELD];
    if call(number::UNAME, [name.as_mut_ptr() as usize, 0, 0, 0, 0, 0]).is_err() {
        return Architecture::Other;
    }
    let field = &name[UTSNAME_MACHINE * UTSNAME_FIELD..][..UTSNAME_FIELD];
    let end = field
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(field.len());
    Architecture::from_machine(&field[..end])
}

fn on_desktop(request: usize, argument: &mut [u8]) -> Result<(), Errno> {
    let fd = call(
        number::OPENAT,
        [
            AT_FDCWD,
            WINDOW_DEVICE.as_ptr() as usize,
            O_RDWR | O_CLOEXEC,
            0,
            0,
            0,
        ],
    )?;
    let asked = call(
        number::IOCTL,
        [fd, request, argument.as_mut_ptr() as usize, 0, 0, 0],
    );
    let _ = call(number::CLOSE, [fd, 0, 0, 0, 0, 0]);
    asked.map(|_| ())
}

/// Tells the desktop the installed apps changed, so its launcher and icons follow at once;
/// nothing without a desktop.
pub fn apps_changed() {
    let _ = on_desktop(APPS_CHANGED, &mut []);
}

/// Asks the desktop to open the installed app `name` as a click on its icon would.
pub fn open_app(name: &[u8]) -> Result<(), Errno> {
    let mut argument = [0u8; OPEN_APP_SIZE];
    let length = name.len().min(OPEN_APP_SIZE - 4);
    argument[..4].copy_from_slice(&(length as u32).to_ne_bytes());
    argument[4..4 + length].copy_from_slice(&name[..length]);
    on_desktop(OPEN_APP, &mut argument)
}

/// Whether standard input is the console, where the console shell runs (D243).
pub fn input_is_console() -> bool {
    let mut target = [0u8; 64];
    let read = call(
        number::READLINKAT,
        [
            AT_FDCWD,
            STANDARD_INPUT_LINK.as_ptr() as usize,
            target.as_mut_ptr() as usize,
            target.len(),
            0,
            0,
        ],
    );
    read.is_ok_and(|length| target.get(..length) == Some(CONSOLE))
}

/// Replaces this program with the app `name` as the desktop would launch it, so it runs in the
/// foreground of the shell that started `app open` (D243): a windowed app's program with its
/// arguments, a console app's line through the shell. Only returns, with the error, when it
/// cannot: `ENOENT` for no such app, `ENXIO` with no desktop.
pub fn run_in_foreground(name: &[u8]) -> Errno {
    let mut argument = [0u8; GET_LAUNCH_SIZE];
    let length = name.len().min(OPEN_APP_SIZE - 4);
    argument[..4].copy_from_slice(&(length as u32).to_ne_bytes());
    argument[4..4 + length].copy_from_slice(&name[..length]);
    if let Err(error) = on_desktop(GET_LAUNCH, &mut argument) {
        return error;
    }
    let word = |at: usize| {
        u32::from_ne_bytes([
            argument[at],
            argument[at + 1],
            argument[at + 2],
            argument[at + 3],
        ])
    };
    let kind = word(0);
    let length = (word(4) as usize).min(GET_LAUNCH_SIZE - LAUNCH_COMMAND);
    let command = &argument[LAUNCH_COMMAND..LAUNCH_COMMAND + length];
    let mut words: [Option<CPath>; MAX_ARGUMENTS] = [const { None }; MAX_ARGUMENTS];
    let mut count = 0;
    let mut push = |word: &[u8]| -> Result<(), Errno> {
        let slot = words.get_mut(count).ok_or(Errno::E2BIG)?;
        *slot = Some(CPath::new(word)?);
        count += 1;
        Ok(())
    };
    let pushed = match kind {
        LAUNCH_START => command
            .split(|&byte| byte == 0)
            .filter(|word| !word.is_empty())
            .try_for_each(&mut push),
        LAUNCH_TYPE => push(SHELL)
            .and_then(|()| push(b"-c"))
            .and_then(|()| push(command)),
        _ => Err(Errno::EINVAL),
    };
    if let Err(error) = pushed {
        return error;
    }
    let mut pointers = [0usize; MAX_ARGUMENTS + 1];
    for (slot, word) in pointers.iter_mut().zip(words.iter().flatten()) {
        *slot = word.pointer();
    }
    if count == 0 {
        return Errno::ENOENT;
    }
    let environment = [0usize];
    match call(
        number::EXECVE,
        [
            pointers[0],
            pointers.as_ptr() as usize,
            environment.as_ptr() as usize,
            0,
            0,
            0,
        ],
    ) {
        Ok(_) => Errno::EIO,
        Err(error) => error,
    }
}

/// Replaces this program with the installer's window, given `arguments` after its name; only
/// returns, with the error, when it cannot.
pub fn start_window(startup: &Startup, arguments: &[&CStr]) -> Errno {
    let mut pointers = [0usize; MAX_ARGUMENTS + 2];
    pointers[0] = WINDOW_PROGRAM.as_ptr() as usize;
    if arguments.len() > MAX_ARGUMENTS {
        return Errno::E2BIG;
    }
    for (slot, argument) in pointers[1..].iter_mut().zip(arguments) {
        *slot = argument.as_ptr() as usize;
    }
    let mut environment = [0usize; MAX_ARGUMENTS + 1];
    for (slot, variable) in environment[..MAX_ARGUMENTS]
        .iter_mut()
        .zip(startup.environment())
    {
        *slot = variable.as_ptr() as usize;
    }
    match call(
        number::EXECVE,
        [
            WINDOW_PROGRAM.as_ptr() as usize,
            pointers.as_ptr() as usize,
            environment.as_ptr() as usize,
            0,
            0,
            0,
        ],
    ) {
        Ok(_) => Errno::EIO,
        Err(error) => error,
    }
}

fn close_descriptor(descriptor: usize) {
    let _ = call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
}

/// Runs `layer --translate program` (D191) with its standard output read into `output`: how
/// many bytes it printed that fit (the rest is read and dropped), and whether it exited with
/// status 0. Its standard error stays the installer's.
pub fn run_translation(
    layer: &[u8],
    program: &[u8],
    output: &mut [u8],
) -> Result<(usize, bool), Errno> {
    let layer = CPath::new(layer)?;
    let option = CPath::new(LAYER_OPTION.as_bytes())?;
    let program = CPath::new(program)?;
    let arguments = [layer.pointer(), option.pointer(), program.pointer(), 0];
    let environment = [0usize];
    let mut ends = [0i32; 2];
    call(
        number::PIPE2,
        [ends.as_mut_ptr() as usize, O_CLOEXEC, 0, 0, 0, 0],
    )?;
    let (reading, writing) = (ends[0] as usize, ends[1] as usize);
    let child = match call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]) {
        Ok(child) => child,
        Err(error) => {
            close_descriptor(reading);
            close_descriptor(writing);
            return Err(error);
        }
    };
    if child == 0 {
        let _ = call(number::DUP3, [writing, STANDARD_OUTPUT, 0, 0, 0, 0]);
        let _ = call(
            number::EXECVE,
            [
                layer.pointer(),
                arguments.as_ptr() as usize,
                environment.as_ptr() as usize,
                0,
                0,
                0,
            ],
        );
        syscall::exit(EXEC_FAILED);
    }
    close_descriptor(writing);
    let mut filled = 0;
    let mut dropped = [0u8; 256];
    loop {
        let fits = filled < output.len();
        let room: &mut [u8] = match fits {
            true => &mut output[filled..],
            false => &mut dropped,
        };
        match call(
            number::READ,
            [reading, room.as_mut_ptr() as usize, room.len(), 0, 0, 0],
        ) {
            Ok(0) => break,
            Ok(count) if fits => filled += count,
            Ok(_) | Err(Errno::EINTR) => {}
            Err(_) => break,
        }
    }
    close_descriptor(reading);
    let mut status = 0i32;
    loop {
        match call(
            number::WAIT4,
            [child, (&raw mut status) as usize, 0, 0, 0, 0],
        ) {
            Ok(_) => return Ok((filled, status == 0)),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(error),
        }
    }
}

/// The most arguments [`run_with_output`] passes.
pub const MOST_ARGUMENTS: usize = 32;
const ARGUMENT_BYTES: usize = 8192;

/// Runs `program` with `arguments` (the program's name first) and an empty environment, its
/// standard output read into `output` (what does not fit is read and dropped): how many bytes
/// it printed that fit, and its exit status (D392).
pub fn run_with_output(
    program: &[u8],
    arguments: &[&[u8]],
    output: &mut [u8],
) -> Result<(usize, i32), Errno> {
    if arguments.len() >= MOST_ARGUMENTS {
        return Err(Errno::E2BIG);
    }
    let program = CPath::new(program)?;
    let mut arena = [0u8; ARGUMENT_BYTES];
    let mut starts = [0usize; MOST_ARGUMENTS];
    let mut used = 0;
    for (start, argument) in starts.iter_mut().zip(arguments) {
        let end = used + argument.len() + 1;
        if end > ARGUMENT_BYTES || argument.contains(&0) {
            return Err(Errno::E2BIG);
        }
        arena[used..end - 1].copy_from_slice(argument);
        *start = used;
        used = end;
    }
    let mut pointers = [0usize; MOST_ARGUMENTS];
    for (pointer, &start) in pointers.iter_mut().zip(&starts[..arguments.len()]) {
        *pointer = arena[start..].as_ptr() as usize;
    }
    let environment = [0usize];
    let mut ends = [0i32; 2];
    call(
        number::PIPE2,
        [ends.as_mut_ptr() as usize, O_CLOEXEC, 0, 0, 0, 0],
    )?;
    let (reading, writing) = (ends[0] as usize, ends[1] as usize);
    let child = match call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]) {
        Ok(child) => child,
        Err(error) => {
            close_descriptor(reading);
            close_descriptor(writing);
            return Err(error);
        }
    };
    if child == 0 {
        let _ = call(number::DUP3, [writing, STANDARD_OUTPUT, 0, 0, 0, 0]);
        let _ = call(
            number::EXECVE,
            [
                program.pointer(),
                pointers.as_ptr() as usize,
                environment.as_ptr() as usize,
                0,
                0,
                0,
            ],
        );
        syscall::exit(EXEC_FAILED);
    }
    close_descriptor(writing);
    let mut filled = 0;
    let mut dropped = [0u8; 256];
    loop {
        let fits = filled < output.len();
        let room: &mut [u8] = match fits {
            true => &mut output[filled..],
            false => &mut dropped,
        };
        match call(
            number::READ,
            [reading, room.as_mut_ptr() as usize, room.len(), 0, 0, 0],
        ) {
            Ok(0) => break,
            Ok(count) if fits => filled += count,
            Ok(_) | Err(Errno::EINTR) => {}
            Err(_) => break,
        }
    }
    close_descriptor(reading);
    let mut status = 0i32;
    loop {
        match call(
            number::WAIT4,
            [child, (&raw mut status) as usize, 0, 0, 0, 0],
        ) {
            Ok(_) => return Ok((filled, (status >> 8) & 0xff)),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(error),
        }
    }
}

struct Cursor {
    directory: Path,
    fd: usize,
    next_index: usize,
    buffer: [u8; DIRECTORY_BUFFER],
    filled: usize,
    offset: usize,
}

/// The file system through Linux's system calls. One directory stays open between
/// [`Files::entry`] calls, so listing a directory in order reads it once.
pub struct LinuxFiles {
    cursor: Option<Cursor>,
}

impl core::fmt::Debug for LinuxFiles {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("LinuxFiles")
    }
}

impl Default for LinuxFiles {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxFiles {
    /// No directory open yet.
    pub const fn new() -> Self {
        Self { cursor: None }
    }

    fn forget_cursor(&mut self) {
        if let Some(cursor) = self.cursor.take() {
            let _ = call(number::CLOSE, [cursor.fd, 0, 0, 0, 0, 0]);
        }
    }

    fn forget_cursor_under(&mut self, path: &[u8]) {
        let affected = self.cursor.as_ref().is_some_and(|cursor| {
            path.starts_with(cursor.directory.as_bytes())
                || cursor.directory.as_bytes().starts_with(path)
        });
        if affected {
            self.forget_cursor();
        }
    }

    fn open_cursor(&mut self, directory: &[u8]) -> Result<(), Errno> {
        self.forget_cursor();
        let path = CPath::new(directory)?;
        let fd = call(
            number::OPENAT,
            [
                AT_FDCWD,
                path.pointer(),
                O_RDONLY | O_DIRECTORY | O_CLOEXEC,
                0,
                0,
                0,
            ],
        )?;
        self.cursor = Some(Cursor {
            directory: Path::from(directory).ok_or(Errno::ENAMETOOLONG)?,
            fd,
            next_index: 0,
            buffer: [0; DIRECTORY_BUFFER],
            filled: 0,
            offset: 0,
        });
        Ok(())
    }

    fn next_name(&mut self, name: &mut [u8; 256]) -> Result<Option<usize>, Errno> {
        let Some(cursor) = self.cursor.as_mut() else {
            return Err(BAD_DESCRIPTOR);
        };
        loop {
            if cursor.offset >= cursor.filled {
                let read = call(
                    number::GETDENTS64,
                    [
                        cursor.fd,
                        cursor.buffer.as_mut_ptr() as usize,
                        DIRECTORY_BUFFER,
                        0,
                        0,
                        0,
                    ],
                )?;
                if read == 0 {
                    return Ok(None);
                }
                cursor.filled = read.min(DIRECTORY_BUFFER);
                cursor.offset = 0;
            }
            let record = &cursor.buffer[cursor.offset..cursor.filled];
            let length = record
                .get(DIRENT_LENGTH..DIRENT_LENGTH + 2)
                .map_or(0, |bytes| {
                    usize::from(u16::from_ne_bytes([bytes[0], bytes[1]]))
                });
            if length <= DIRENT_NAME || length > record.len() {
                cursor.offset = cursor.filled;
                return Err(Errno::EIO);
            }
            cursor.offset += length;
            let raw = &record[DIRENT_NAME..length];
            let file = &raw[..raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len())];
            if file == b"." || file == b".." || file.is_empty() || file.len() > name.len() {
                continue;
            }
            name[..file.len()].copy_from_slice(file);
            return Ok(Some(file.len()));
        }
    }
}

impl Drop for LinuxFiles {
    fn drop(&mut self) {
        self.forget_cursor();
    }
}

fn space_of(record: &[u8; STATFS_SIZE]) -> Space {
    let word = |at: usize| {
        let mut value = [0u8; 8];
        value.copy_from_slice(&record[at..at + 8]);
        u64::from_ne_bytes(value)
    };
    let block = match word(STATFS_FRAGMENT_SIZE) {
        0 => word(STATFS_BLOCK_SIZE),
        fragment => fragment,
    };
    Space {
        total: word(STATFS_BLOCKS).saturating_mul(block),
        available: word(STATFS_AVAILABLE).saturating_mul(block),
        read_only: word(STATFS_FLAGS) & STATFS_READ_ONLY != 0,
    }
}

fn status_with(path: &[u8], flags: usize) -> Result<Status, Errno> {
    let path = CPath::new(path)?;
    let mut record = [0u8; STATX_SIZE];
    call(
        number::STATX,
        [
            AT_FDCWD,
            path.pointer(),
            flags,
            STATX_BASIC_STATS,
            record.as_mut_ptr() as usize,
            0,
        ],
    )?;
    let mode = u32::from(u16::from_ne_bytes([
        record[STATX_MODE],
        record[STATX_MODE + 1],
    ]));
    let double_word =
        |at: usize| u64::from_ne_bytes(record[at..at + 8].try_into().unwrap_or([0; 8]));
    let word = |at: usize| u32::from_ne_bytes(record[at..at + 4].try_into().unwrap_or([0; 4]));
    let size = double_word(STATX_FILE_SIZE);
    let kind = match mode & FILE_TYPE {
        REGULAR => Kind::File,
        DIRECTORY => Kind::Directory,
        SYMBOLIC_LINK => Kind::Link,
        _ => Kind::Other,
    };
    Ok(Status {
        kind,
        size,
        permissions: mode & 0o7777,
        modified: double_word(STATX_MODIFIED).min(i64::MAX as u64),
        device: u64::from(word(STATX_DEVICE_MAJOR)) << 32 | u64::from(word(STATX_DEVICE_MINOR)),
    })
}

impl Files for LinuxFiles {
    fn status(&mut self, path: &[u8]) -> Result<Status, Errno> {
        status_with(path, AT_SYMLINK_NOFOLLOW)
    }

    fn space(&mut self, path: &[u8]) -> Result<Space, Errno> {
        let path = CPath::new(path)?;
        let mut record = [0u8; STATFS_SIZE];
        call(
            number::STATFS,
            [path.pointer(), record.as_mut_ptr() as usize, 0, 0, 0, 0],
        )?;
        Ok(space_of(&record))
    }

    fn status_following(&mut self, path: &[u8]) -> Result<Status, Errno> {
        status_with(path, 0)
    }

    fn may_write(&mut self, path: &[u8]) -> Result<(), Errno> {
        let path = CPath::new(path)?;
        call(
            number::FACCESSAT,
            [AT_FDCWD, path.pointer(), WRITE_ACCESS, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn read_link(&mut self, path: &[u8], target: &mut [u8]) -> Result<usize, Errno> {
        let path = CPath::new(path)?;
        let length = call(
            number::READLINKAT,
            [
                AT_FDCWD,
                path.pointer(),
                target.as_mut_ptr() as usize,
                target.len(),
                0,
                0,
            ],
        )?;
        match length < target.len() {
            true => Ok(length),
            false => Err(Errno::ENAMETOOLONG),
        }
    }

    fn make_link(&mut self, target: &[u8], path: &[u8]) -> Result<(), Errno> {
        self.forget_cursor_under(path);
        let (target, path) = (CPath::new(target)?, CPath::new(path)?);
        call(
            number::SYMLINKAT,
            [target.pointer(), AT_FDCWD, path.pointer(), 0, 0, 0],
        )
        .map(|_| ())
    }

    fn entry(
        &mut self,
        directory: &[u8],
        index: usize,
        name: &mut [u8; 256],
    ) -> Result<Option<usize>, Errno> {
        let continues = self.cursor.as_ref().is_some_and(|cursor| {
            cursor.directory.as_bytes() == directory && cursor.next_index == index
        });
        if !continues {
            self.open_cursor(directory)?;
            for _ in 0..index {
                if self.next_name(name)?.is_none() {
                    return Ok(None);
                }
            }
            if let Some(cursor) = self.cursor.as_mut() {
                cursor.next_index = index;
            }
        }
        let found = self.next_name(name)?;
        if let Some(cursor) = self.cursor.as_mut() {
            cursor.next_index += 1;
        }
        Ok(found)
    }

    fn open(&mut self, path: &[u8]) -> Result<Handle, Errno> {
        let path = CPath::new(path)?;
        call(
            number::OPENAT,
            [AT_FDCWD, path.pointer(), O_RDONLY | O_CLOEXEC, 0, 0, 0],
        )
    }

    fn create(&mut self, path: &[u8], permissions: u32) -> Result<Handle, Errno> {
        self.forget_cursor_under(path);
        let path = CPath::new(path)?;
        call(
            number::OPENAT,
            [
                AT_FDCWD,
                path.pointer(),
                O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC,
                permissions as usize,
                0,
                0,
            ],
        )
    }

    fn read(&mut self, file: Handle, buffer: &mut [u8]) -> Result<usize, Errno> {
        loop {
            match call(
                number::READ,
                [file, buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0],
            ) {
                Err(Errno::EINTR) => {}
                other => return other,
            }
        }
    }

    fn read_at(&mut self, file: Handle, offset: u64, buffer: &mut [u8]) -> Result<usize, Errno> {
        let offset = usize::try_from(offset).map_err(|_| Errno::EINVAL)?;
        loop {
            match call(
                number::PREAD64,
                [
                    file,
                    buffer.as_mut_ptr() as usize,
                    buffer.len(),
                    offset,
                    0,
                    0,
                ],
            ) {
                Err(Errno::EINTR) => {}
                other => return other,
            }
        }
    }

    fn write(&mut self, file: Handle, mut bytes: &[u8]) -> Result<(), Errno> {
        while !bytes.is_empty() {
            match call(
                number::WRITE,
                [file, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
            ) {
                Ok(0) => return Err(Errno::EIO),
                Ok(written) => bytes = &bytes[written.min(bytes.len())..],
                Err(Errno::EINTR) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn sync_file(&mut self, file: Handle) -> Result<(), Errno> {
        call(number::FSYNC, [file, 0, 0, 0, 0, 0]).map(|_| ())
    }

    fn close(&mut self, file: Handle) {
        let _ = call(number::CLOSE, [file, 0, 0, 0, 0, 0]);
    }

    fn make_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.forget_cursor_under(path);
        let path = CPath::new(path)?;
        call(
            number::MKDIRAT,
            [AT_FDCWD, path.pointer(), DIRECTORY_MODE, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.forget_cursor_under(path);
        let path = CPath::new(path)?;
        call(number::UNLINKAT, [AT_FDCWD, path.pointer(), 0, 0, 0, 0]).map(|_| ())
    }

    fn remove_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        self.forget_cursor_under(path);
        let path = CPath::new(path)?;
        call(
            number::UNLINKAT,
            [AT_FDCWD, path.pointer(), AT_REMOVEDIR, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), Errno> {
        self.forget_cursor_under(from);
        self.forget_cursor_under(to);
        let (from, to) = (CPath::new(from)?, CPath::new(to)?);
        call(
            number::RENAMEAT2,
            [AT_FDCWD, from.pointer(), AT_FDCWD, to.pointer(), 0, 0],
        )
        .map(|_| ())
    }

    fn sync(&mut self) {
        let _ = call(number::SYNC, [0; 6]);
    }
}
