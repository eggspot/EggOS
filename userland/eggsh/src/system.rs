use crate::arguments::MAX_ARGUMENTS;
use crate::editor::{
    Editor, Fed, HISTORY_CAPACITY, History, HistoryLoader, LOADED_LINE_CAPACITY, should_remember,
};
use crate::path::{Files, Kind, PATH_CAPACITY};
use crate::shell::{Errno, Forked, Group, Read, Shell, Streams, System, Wait};
use crate::status::USAGE;
use crate::syntax::{MAX_WORDS, Redirection};
use crate::test::{Access, FileStatus, FileType};
use crate::variables::MAX_VARIABLES;
use core::cell::UnsafeCell;
use core::ffi::CStr;
use core::ptr;
use core::sync::atomic::{AtomicIsize, Ordering};
use eggos_runtime::process::{self, CLONE_VFORK, CLONE_VM};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};

const TERMINAL: usize = 0;
const HISTORY_FILE_LIMIT: usize = 8 * HISTORY_CAPACITY;
const STANDARD_ERROR: usize = 2;
const AT_FDCWD: usize = -100isize as usize;
const O_CLOEXEC: usize = 0o2_000_000;
const O_RDONLY_CLOEXEC: usize = O_CLOEXEC;
const O_WRONLY: usize = 0o1;
const O_CREAT: usize = 0o100;
const O_TRUNC: usize = 0o1000;
const O_APPEND: usize = 0o2000;
const NEW_FILE_MODE: usize = 0o666;
const HISTORY_FILE_MODE: usize = 0o600;
const HISTORY_PIECE: usize = 1024;
const TEMPORARY_SUFFIX: &[u8] = b".tmp";
const F_DUPFD_CLOEXEC: usize = 1030;
const WNOHANG: usize = 1;
const WUNTRACED: usize = 2;
const WCONTINUED: usize = 8;
const SIGPIPE: usize = 13;
const EINTR: isize = 4;
const ENOENT: isize = 2;
const SIGINT: usize = 2;
const SIGCHLD: usize = 17;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
#[cfg(target_arch = "x86_64")]
const SA_RESTORER: usize = 0x0400_0000;
const SIGNAL_SET_SIZE: usize = 8;
const JOB_CONTROL_SIGNALS: [usize; 4] = [3, 20, 21, 22];
const SIG_BLOCK: usize = 0;
const SIG_SETMASK: usize = 2;
const INTERRUPT_MASK: u64 = 1 << (SIGINT - 1);
const POLLIN: i16 = 1;
const TCGETS: usize = 0x5401;
const TCSETS: usize = 0x5402;
const LOCAL_FLAGS: usize = 12;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const CONTROL_CHARACTERS: usize = 17;
const VTIME: usize = 5;
const VMIN: usize = 6;
const TIOCSPGRP: usize = 0x5410;
const TERMIOS_SIZE: usize = 64;
const STAT_SIZE: usize = 144;
const MODE_OFFSET: usize = if cfg!(target_arch = "x86_64") { 24 } else { 16 };
const FILE_TYPE: u32 = 0o170_000;
const DIRECTORY: u32 = 0o040_000;
const REGULAR_FILE: u32 = 0o100_000;
const ANY_EXECUTE: u32 = 0o111;
const CHILD_STACK_SIZE: usize = 16 * 1024;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const SIZE_OFFSET: usize = 48;
const SYMBOLIC_LINK: u32 = 0o120_000;
const CHARACTER_DEVICE: u32 = 0o020_000;
const BLOCK_DEVICE: u32 = 0o060_000;
const FIFO: u32 = 0o010_000;
const SOCKET: u32 = 0o140_000;
const MODE_BITS: u32 = 0o7777;
const READ_ACCESS: usize = 4;
const WRITE_ACCESS: usize = 2;
const EXECUTE_ACCESS: usize = 1;
const MFD_CLOEXEC: usize = 1;
const SEEK_SET: usize = 0;
const DOCUMENT_NAME: &[u8] = b"eggsh-here-document\0";
const DIRECTORY_PIECE: usize = 4096;
const DIRECTORY_ENTRY_NAME: usize = 19;
const DIRECTORY_ENTRY_LENGTH: usize = 16;
const COMMAND_NOT_RUN: i32 = 127;
const NOT_EXECUTABLE: i32 = 126;

#[cfg(not(target_arch = "riscv64"))]
#[repr(C)]
struct SignalAction {
    handler: usize,
    flags: usize,
    restorer: usize,
    mask: u64,
}

#[cfg(target_arch = "riscv64")]
#[repr(C)]
struct SignalAction {
    handler: usize,
    flags: usize,
    mask: u64,
}

impl SignalAction {
    fn with_handler(handler: usize) -> Self {
        #[cfg(target_arch = "x86_64")]
        let (flags, restorer) = match handler {
            SIG_DFL | SIG_IGN => (0, 0),
            _ => (
                SA_RESTORER,
                process::restore_after_signal as *const () as usize,
            ),
        };
        #[cfg(target_arch = "aarch64")]
        let (flags, restorer) = (0, 0);
        #[cfg(target_arch = "riscv64")]
        let flags = 0;
        Self {
            handler,
            flags,
            #[cfg(not(target_arch = "riscv64"))]
            restorer,
            mask: 0,
        }
    }
}

#[repr(C, align(16))]
struct ChildStack(UnsafeCell<[u8; CHILD_STACK_SIZE]>);

// SAFETY: only a child started by `Linux::run` uses the stack, while the shell's only thread
// sleeps in its `CLONE_VFORK` clone, and one child at a time.
unsafe impl Sync for ChildStack {}

static CHILD_STACK: ChildStack = ChildStack(UnsafeCell::new([0; CHILD_STACK_SIZE]));

#[repr(C)]
struct PollDescriptor {
    descriptor: i32,
    events: i16,
    returned_events: i16,
}

struct Request {
    program: *const u8,
    arguments: *const *const u8,
    environment: *const *const u8,
    streams: Streams,
    group: Group,
    error: AtomicIsize,
}

fn call(number: usize, arguments: [usize; 6]) -> isize {
    // SAFETY: every call the shell makes passes only memory it owns for the call's length,
    // and none maps, unmaps or protects memory.
    unsafe { syscall::call(number, arguments) }
}

fn set_signal_action(signal: usize, handler: usize) {
    let action = SignalAction::with_handler(handler);
    call(
        number::RT_SIGACTION,
        [signal, &raw const action as usize, 0, SIGNAL_SET_SIZE, 0, 0],
    );
}

extern "C" fn interrupted(_: i32) {}

fn set_foreground(group: i32) {
    call(
        number::IOCTL,
        [TERMINAL, TIOCSPGRP, &raw const group as usize, 0, 0, 0],
    );
}

fn is_terminal(descriptor: usize) -> bool {
    let mut settings = [0u8; TERMIOS_SIZE];
    call(
        number::IOCTL,
        [descriptor, TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    ) == 0
}

fn set_signal_mask(how: usize, mask: u64) {
    call(
        number::RT_SIGPROCMASK,
        [how, &raw const mask as usize, 0, SIGNAL_SET_SIZE, 0, 0],
    );
}

fn wait_for_input_or_interrupt(descriptor: usize) -> bool {
    let mut poll = PollDescriptor {
        descriptor: descriptor as i32,
        events: POLLIN,
        returned_events: 0,
    };
    let nothing_blocked: u64 = 0;
    call(
        number::PPOLL,
        [
            &raw mut poll as usize,
            1,
            0,
            &raw const nothing_blocked as usize,
            SIGNAL_SET_SIZE,
            0,
        ],
    ) != -EINTR
}

fn terminal_settings() -> Option<[u8; TERMIOS_SIZE]> {
    let mut settings = [0u8; TERMIOS_SIZE];
    let got = call(
        number::IOCTL,
        [TERMINAL, TCGETS, settings.as_mut_ptr() as usize, 0, 0, 0],
    );
    (got == 0).then_some(settings)
}

fn set_terminal_settings(settings: &[u8; TERMIOS_SIZE]) {
    call(
        number::IOCTL,
        [TERMINAL, TCSETS, settings.as_ptr() as usize, 0, 0, 0],
    );
}

fn byte_at_a_time(mut settings: [u8; TERMIOS_SIZE]) -> [u8; TERMIOS_SIZE] {
    let mut local = [0; 4];
    local.copy_from_slice(&settings[LOCAL_FLAGS..LOCAL_FLAGS + 4]);
    let local = u32::from_ne_bytes(local) & !(ICANON | ECHO);
    settings[LOCAL_FLAGS..LOCAL_FLAGS + 4].copy_from_slice(&local.to_ne_bytes());
    settings[CONTROL_CHARACTERS + VMIN] = 1;
    settings[CONTROL_CHARACTERS + VTIME] = 0;
    settings
}

fn take_the_terminal() -> i32 {
    set_signal_mask(SIG_BLOCK, INTERRUPT_MASK);
    set_signal_action(SIGINT, interrupted as *const () as usize);
    for signal in JOB_CONTROL_SIGNALS {
        set_signal_action(signal, SIG_IGN);
    }
    let pid = call(number::GETPID, [0; 6]);
    if call(number::GETPGID, [0; 6]) != pid {
        call(number::SETPGID, [0; 6]);
    }
    let group = call(number::GETPGID, [0; 6]) as i32;
    set_foreground(group);
    group
}

extern "C" fn start_child(request: usize) -> ! {
    // SAFETY: `Linux::run` passes its `Request`, which stays as it is while this child runs
    // on the shell's memory: the shell's thread sleeps until the child runs `execve` or ends.
    let request = unsafe { &*(request as *const Request) };
    let job_control = match request.group {
        Group::Shell => false,
        Group::New => {
            call(number::SETPGID, [0; 6]);
            set_foreground(call(number::GETPID, [0; 6]) as i32);
            true
        }
        Group::Background => {
            call(number::SETPGID, [0; 6]);
            true
        }
        Group::Join(leader) => {
            call(number::SETPGID, [0, leader as usize, 0, 0, 0, 0]);
            true
        }
    };
    if job_control {
        set_signal_action(SIGINT, SIG_DFL);
        for signal in JOB_CONTROL_SIGNALS {
            set_signal_action(signal, SIG_DFL);
        }
        set_signal_mask(SIG_SETMASK, 0);
    }
    set_signal_action(SIGPIPE, SIG_DFL);
    let mut sources = [
        request.streams.input,
        request.streams.output,
        request.streams.error,
    ];
    for (target, source) in sources.iter_mut().enumerate() {
        if *source <= STANDARD_ERROR && *source != target {
            let copy = call(number::FCNTL, [*source, F_DUPFD_CLOEXEC, 3, 0, 0, 0]);
            if copy >= 0 {
                *source = copy as usize;
            }
        }
    }
    for (target, &source) in sources.iter().enumerate() {
        if source != target {
            call(number::DUP3, [source, target, 0, 0, 0, 0]);
        }
    }
    let result = call(
        number::EXECVE,
        [
            request.program as usize,
            request.arguments as usize,
            request.environment as usize,
            0,
            0,
            0,
        ],
    );
    request.error.store(-result, Ordering::Release);
    syscall::exit(if result == -ENOENT {
        COMMAND_NOT_RUN
    } else {
        NOT_EXECUTABLE
    })
}

fn wait_for(child: isize, options: usize) -> Result<Option<(i32, i32)>, Errno> {
    loop {
        let mut status = 0i32;
        let changed = call(
            number::WAIT4,
            [child as usize, &raw mut status as usize, options, 0, 0, 0],
        );
        match changed {
            0 => return Ok(None),
            1.. => return Ok(Some((changed as i32, status))),
            _ if changed == -EINTR => {}
            _ => return Err(Errno(changed.unsigned_abs() as u16)),
        }
    }
}

fn open_file(path: &[u8], flags: usize, mode: usize) -> Result<usize, Errno> {
    let descriptor = call(
        number::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags, mode, 0, 0],
    );
    match descriptor {
        0.. => Ok(descriptor as usize),
        _ => Err(Errno(descriptor.unsigned_abs() as u16)),
    }
}

fn close(descriptor: usize) {
    call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
}

fn reset_signals() {
    set_signal_action(SIGINT, SIG_DFL);
    for signal in JOB_CONTROL_SIGNALS {
        set_signal_action(signal, SIG_DFL);
    }
    set_signal_mask(SIG_SETMASK, 0);
}

fn join_group(process: usize, group: Group) {
    let leader = match group {
        Group::Shell => return,
        Group::New | Group::Background => process,
        Group::Join(leader) => leader as usize,
    };
    call(number::SETPGID, [process, leader, 0, 0, 0, 0]);
}

fn file_type(mode: u32) -> FileType {
    match mode & FILE_TYPE {
        DIRECTORY => FileType::Directory,
        SYMBOLIC_LINK => FileType::SymbolicLink,
        CHARACTER_DEVICE => FileType::CharacterDevice,
        BLOCK_DEVICE => FileType::BlockDevice,
        FIFO => FileType::Fifo,
        SOCKET => FileType::Socket,
        _ => FileType::Regular,
    }
}

fn read_raw(descriptor: usize, buffer: &mut [u8]) -> Read {
    let read = call(
        number::READ,
        [
            descriptor,
            buffer.as_mut_ptr() as usize,
            buffer.len(),
            0,
            0,
            0,
        ],
    );
    match read {
        1.. => Read::Bytes(read as usize),
        _ if read == -EINTR => Read::Interrupted,
        _ => Read::End,
    }
}

#[derive(Debug)]
struct HistoryFile {
    path: [u8; PATH_CAPACITY],
    length: usize,
    loaded: bool,
}

impl HistoryFile {
    fn path_with_nul(&self) -> Option<&[u8]> {
        (self.length > 0).then(|| &self.path[..=self.length])
    }

    fn name(&mut self, file: Option<&[u8]>, history: &mut History) {
        let file = file.unwrap_or_default();
        if file == &self.path[..self.length] {
            return;
        }
        if file.len() + TEMPORARY_SUFFIX.len() >= PATH_CAPACITY {
            self.length = 0;
            return;
        }
        self.path[..file.len()].copy_from_slice(file);
        self.path[file.len()] = 0;
        self.length = file.len();
        if self.length > 0 && !self.loaded {
            self.loaded = true;
            self.load(history);
        }
    }

    fn load(&mut self, history: &mut History) {
        let Some(path) = self.path_with_nul() else {
            return;
        };
        let Ok(descriptor) = open_file(path, O_RDONLY_CLOEXEC, 0) else {
            return;
        };
        let mut loader = HistoryLoader::default();
        let mut piece = [0u8; HISTORY_PIECE];
        let mut size = 0;
        loop {
            let read = call(
                number::READ,
                [
                    descriptor,
                    piece.as_mut_ptr() as usize,
                    piece.len(),
                    0,
                    0,
                    0,
                ],
            );
            if read <= 0 {
                break;
            }
            size += read as usize;
            loader.feed(&piece[..read as usize], history);
        }
        loader.finish(history);
        close(descriptor);
        if size > HISTORY_FILE_LIMIT {
            self.write_again(history);
        }
    }

    fn write_again(&self, history: &History) {
        let mut temporary = [0u8; PATH_CAPACITY];
        let end = self.length + TEMPORARY_SUFFIX.len();
        temporary[..self.length].copy_from_slice(&self.path[..self.length]);
        temporary[self.length..end].copy_from_slice(TEMPORARY_SUFFIX);
        let flags = O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC;
        let Ok(descriptor) = open_file(&temporary[..=end], flags, HISTORY_FILE_MODE) else {
            return;
        };
        let mut line = [0u8; LOADED_LINE_CAPACITY + 1];
        for back in (0..history.len()).rev() {
            if let Some(kept) = history.get(back)
                && kept.len() < line.len()
            {
                line[..kept.len()].copy_from_slice(kept);
                line[kept.len()] = b'\n';
                syscall::write_all(descriptor, &line[..=kept.len()]);
            }
        }
        close(descriptor);
        call(
            number::RENAMEAT2,
            [
                AT_FDCWD,
                temporary.as_ptr() as usize,
                AT_FDCWD,
                self.path.as_ptr() as usize,
                0,
                0,
            ],
        );
    }

    fn append(&self, line: &[u8]) {
        let Some(path) = self.path_with_nul() else {
            return;
        };
        let flags = O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC;
        let Ok(descriptor) = open_file(path, flags, HISTORY_FILE_MODE) else {
            return;
        };
        let mut kept = [0u8; LOADED_LINE_CAPACITY + 1];
        if line.len() < kept.len() {
            kept[..line.len()].copy_from_slice(line);
            kept[line.len()] = b'\n';
            syscall::write_all(descriptor, &kept[..=line.len()]);
        }
        close(descriptor);
    }
}

#[derive(Debug)]
struct Linux {
    group: i32,
    settings: Option<[u8; TERMIOS_SIZE]>,
    editing: bool,
    editor: Editor,
    history: History,
    history_file: HistoryFile,
}

impl Linux {
    fn has_the_terminal(&self) -> bool {
        self.group != 0
    }

    fn edit_lines(&mut self, editing: bool) {
        let Some(settings) = self.settings else {
            return;
        };
        if self.editing != editing {
            self.editing = editing;
            match editing {
                true => set_terminal_settings(&byte_at_a_time(settings)),
                false => set_terminal_settings(&settings),
            }
        }
    }

    fn read_edited_line(&mut self, buffer: &mut [u8]) -> Read {
        self.edit_lines(true);
        loop {
            if !wait_for_input_or_interrupt(TERMINAL) {
                return self.interrupted();
            }
            let mut byte = 0u8;
            let read = call(number::READ, [TERMINAL, &raw mut byte as usize, 1, 0, 0, 0]);
            match read {
                1 => {}
                _ if read == -EINTR => return self.interrupted(),
                _ => return Read::End,
            }
            let fed = self.editor.feed(byte, buffer, &self.history, &mut |echo| {
                syscall::write_all(STANDARD_ERROR, echo);
            });
            match fed {
                Fed::Editing => {}
                Fed::Line(length) => {
                    let line = &buffer[..length - 1];
                    if should_remember(line) && self.history.add(line) {
                        self.history_file.append(line);
                    }
                    return Read::Bytes(length);
                }
                Fed::End => return Read::End,
            }
        }
    }

    fn interrupted(&mut self) -> Read {
        self.editor = Editor::new();
        syscall::write_all(STANDARD_ERROR, b"^C");
        Read::Interrupted
    }
}

impl Files for Linux {
    fn kind(&mut self, path: &[u8]) -> Kind {
        let mut status = [0u8; STAT_SIZE];
        let result = call(
            number::NEWFSTATAT,
            [
                AT_FDCWD,
                path.as_ptr() as usize,
                status.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        );
        if result < 0 {
            return Kind::Missing;
        }
        let mut mode = [0; 4];
        mode.copy_from_slice(&status[MODE_OFFSET..MODE_OFFSET + 4]);
        let mode = u32::from_ne_bytes(mode);
        match mode & FILE_TYPE {
            DIRECTORY => Kind::Directory,
            REGULAR_FILE if mode & ANY_EXECUTE != 0 => Kind::Executable,
            _ => Kind::NotExecutable,
        }
    }
}

impl System for Linux {
    fn write(&mut self, descriptor: usize, bytes: &[u8]) {
        syscall::write_all(descriptor, bytes);
    }

    fn read(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read {
        if descriptor == TERMINAL && self.has_the_terminal() && self.settings.is_some() {
            return self.read_edited_line(buffer);
        }
        if descriptor == TERMINAL
            && self.has_the_terminal()
            && !wait_for_input_or_interrupt(descriptor)
        {
            return Read::Interrupted;
        }
        read_raw(descriptor, buffer)
    }

    fn read_data(&mut self, descriptor: usize, buffer: &mut [u8]) -> Read {
        if descriptor == TERMINAL {
            self.edit_lines(false);
        }
        read_raw(descriptor, buffer)
    }

    fn open(&mut self, path: &[u8]) -> Result<usize, Errno> {
        let descriptor = call(
            number::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, O_RDONLY_CLOEXEC, 0, 0, 0],
        );
        match descriptor {
            0.. => Ok(descriptor as usize),
            _ => Err(Errno(descriptor.unsigned_abs() as u16)),
        }
    }

    fn close(&mut self, descriptor: usize) {
        call(number::CLOSE, [descriptor, 0, 0, 0, 0, 0]);
    }

    fn change_directory(&mut self, path: &[u8]) -> Result<(), Errno> {
        match call(number::CHDIR, [path.as_ptr() as usize, 0, 0, 0, 0, 0]) {
            0 => Ok(()),
            error => Err(Errno(error.unsigned_abs() as u16)),
        }
    }

    fn working_directory<'b>(&mut self, buffer: &'b mut [u8; PATH_CAPACITY]) -> Option<&'b [u8]> {
        let result = call(
            number::GETCWD,
            [buffer.as_mut_ptr() as usize, buffer.len(), 0, 0, 0, 0],
        );
        if result <= 0 {
            return None;
        }
        let end = buffer.iter().position(|&byte| byte == 0)?;
        Some(&buffer[..end])
    }

    fn open_redirection(&mut self, path: &[u8], redirection: Redirection) -> Result<usize, Errno> {
        let flags = O_CLOEXEC
            | match redirection {
                Redirection::Input => 0,
                Redirection::Output => O_WRONLY | O_CREAT | O_TRUNC,
                Redirection::Append => O_WRONLY | O_CREAT | O_APPEND,
            };
        let descriptor = call(
            number::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, flags, NEW_FILE_MODE, 0, 0],
        );
        match descriptor {
            0.. => Ok(descriptor as usize),
            _ => Err(Errno(descriptor.unsigned_abs() as u16)),
        }
    }

    fn document(&mut self, contents: &[u8]) -> Result<usize, Errno> {
        let descriptor = call(
            number::MEMFD_CREATE,
            [DOCUMENT_NAME.as_ptr() as usize, MFD_CLOEXEC, 0, 0, 0, 0],
        );
        if descriptor < 0 {
            return Err(Errno(descriptor.unsigned_abs() as u16));
        }
        let descriptor = descriptor as usize;
        if !syscall::write_all(descriptor, contents) {
            close(descriptor);
            return Err(Errno::ENOSPC);
        }
        call(number::LSEEK, [descriptor, 0, SEEK_SET, 0, 0, 0]);
        Ok(descriptor)
    }

    fn pipe(&mut self) -> Result<(usize, usize), Errno> {
        let mut ends = [0i32; 2];
        match call(
            number::PIPE2,
            [ends.as_mut_ptr() as usize, O_CLOEXEC, 0, 0, 0, 0],
        ) {
            0 => Ok((ends[0] as usize, ends[1] as usize)),
            error => Err(Errno(error.unsigned_abs() as u16)),
        }
    }

    fn wait(&mut self, child: i32, how: Wait) -> Result<Option<(i32, i32)>, Errno> {
        let job_control = match self.has_the_terminal() {
            true => WUNTRACED,
            false => 0,
        };
        let options = match how {
            Wait::Block => job_control,
            Wait::Poll if job_control != 0 => WNOHANG | WUNTRACED | WCONTINUED,
            Wait::Poll => WNOHANG,
        };
        wait_for(child as isize, options)
    }

    fn take_back_terminal(&mut self) {
        if self.has_the_terminal() {
            set_foreground(self.group);
            self.edit_lines(true);
        }
    }

    fn give_terminal(&mut self, group: i32) {
        if self.has_the_terminal() && group != 0 {
            self.edit_lines(false);
            set_foreground(group);
        }
    }

    fn kill(&mut self, target: i32, signal: u8) -> Result<(), Errno> {
        match call(
            number::KILL,
            [target as isize as usize, usize::from(signal), 0, 0, 0, 0],
        ) {
            0 => Ok(()),
            error => Err(Errno(error.unsigned_abs() as u16)),
        }
    }

    fn keep_history_in(&mut self, file: Option<&[u8]>) {
        if self.settings.is_some() {
            self.history_file.name(file, &mut self.history);
        }
    }

    fn fork(&mut self, group: Group) -> Result<Forked, Errno> {
        self.edit_lines(false);
        let child = call(number::CLONE, [SIGCHLD, 0, 0, 0, 0, 0]);
        match child {
            0 => {
                let job_control = self.has_the_terminal();
                join_group(0, group);
                if group == Group::New {
                    set_foreground(call(number::GETPID, [0; 6]) as i32);
                }
                if job_control {
                    reset_signals();
                }
                set_signal_action(SIGPIPE, SIG_DFL);
                self.group = 0;
                self.settings = None;
                Ok(Forked::Child)
            }
            1.. => {
                join_group(child as usize, group);
                Ok(Forked::Parent(child as i32))
            }
            _ => Err(Errno(child.unsigned_abs() as u16)),
        }
    }

    fn end_subshell(&mut self, status: i32) {
        syscall::exit(status)
    }

    fn read_directory(&mut self, path: &[u8], each: &mut dyn FnMut(&[u8])) -> Result<(), Errno> {
        let descriptor = open_file(path, O_RDONLY_CLOEXEC, 0)?;
        let mut piece = [0u8; DIRECTORY_PIECE];
        loop {
            let read = call(
                number::GETDENTS64,
                [
                    descriptor,
                    piece.as_mut_ptr() as usize,
                    piece.len(),
                    0,
                    0,
                    0,
                ],
            );
            if read <= 0 {
                close(descriptor);
                return match read {
                    0 => Ok(()),
                    _ => Err(Errno(read.unsigned_abs() as u16)),
                };
            }
            let mut at = 0;
            while at + DIRECTORY_ENTRY_NAME <= read as usize {
                let length = usize::from(u16::from_ne_bytes([
                    piece[at + DIRECTORY_ENTRY_LENGTH],
                    piece[at + DIRECTORY_ENTRY_LENGTH + 1],
                ]));
                if length <= DIRECTORY_ENTRY_NAME || at + length > read as usize {
                    break;
                }
                let name = &piece[at + DIRECTORY_ENTRY_NAME..at + length];
                let end = name
                    .iter()
                    .position(|&byte| byte == 0)
                    .unwrap_or(name.len());
                each(&name[..end]);
                at += length;
            }
        }
    }

    fn file_status(&mut self, path: &[u8], follow: bool) -> Option<FileStatus> {
        let mut status = [0u8; STAT_SIZE];
        let flags = if follow { 0 } else { AT_SYMLINK_NOFOLLOW };
        let result = call(
            number::NEWFSTATAT,
            [
                AT_FDCWD,
                path.as_ptr() as usize,
                status.as_mut_ptr() as usize,
                flags,
                0,
                0,
            ],
        );
        if result < 0 {
            return None;
        }
        let mode = u32::from_ne_bytes(status[MODE_OFFSET..MODE_OFFSET + 4].try_into().ok()?);
        let size = u64::from_ne_bytes(status[SIZE_OFFSET..SIZE_OFFSET + 8].try_into().ok()?);
        Some(FileStatus {
            file_type: file_type(mode),
            size,
            mode: mode & MODE_BITS,
        })
    }

    fn accessible(&mut self, path: &[u8], access: Access) -> bool {
        let mode = match access {
            Access::Read => READ_ACCESS,
            Access::Write => WRITE_ACCESS,
            Access::Execute => EXECUTE_ACCESS,
        };
        call(
            number::FACCESSAT,
            [AT_FDCWD, path.as_ptr() as usize, mode, 0, 0, 0],
        ) == 0
    }

    fn is_terminal(&mut self, descriptor: usize) -> bool {
        is_terminal(descriptor)
    }

    fn start(
        &mut self,
        program: &[u8],
        arguments: &[&[u8]],
        environment: &[&[u8]],
        streams: Streams,
        group: Group,
    ) -> Result<i32, Errno> {
        if arguments.len() > MAX_WORDS || environment.len() > MAX_VARIABLES + MAX_WORDS {
            return Err(Errno::E2BIG);
        }
        self.edit_lines(false);
        let mut argument_pointers = [ptr::null::<u8>(); MAX_WORDS + 1];
        for (slot, argument) in argument_pointers.iter_mut().zip(arguments) {
            *slot = argument.as_ptr();
        }
        let mut environment_pointers = [ptr::null::<u8>(); MAX_VARIABLES + MAX_WORDS + 1];
        for (slot, variable) in environment_pointers.iter_mut().zip(environment) {
            *slot = variable.as_ptr();
        }
        let request = Request {
            program: program.as_ptr(),
            arguments: argument_pointers.as_ptr(),
            environment: environment_pointers.as_ptr(),
            streams,
            group,
            error: AtomicIsize::new(0),
        };
        let stack_top = CHILD_STACK.0.get() as usize + CHILD_STACK_SIZE;
        // SAFETY: the child runs `start_child` on `CHILD_STACK`, which nothing else uses, and
        // only reads `request` and the strings it names and writes its `error`, all of which
        // stay in place: this thread sleeps until the child runs `execve` or ends.
        let child = unsafe {
            process::clone_onto_stack(
                CLONE_VM | CLONE_VFORK | SIGCHLD,
                stack_top,
                start_child,
                &raw const request as usize,
            )
        };
        if child < 0 {
            return Err(Errno(child.unsigned_abs() as u16));
        }
        match request.error.load(Ordering::Acquire) {
            0 => Ok(child as i32),
            error => {
                let _ = wait_for(child, 0);
                if group == Group::New {
                    self.take_back_terminal();
                }
                Err(Errno(error as u16))
            }
        }
    }
}

/// The shell's main function: see the crate's documentation.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = startup.arguments();
    let name = arguments.next().map_or(&b"eggsh"[..], CStr::to_bytes);
    let mut positional: [&[u8]; MAX_ARGUMENTS + 1] = [b""; MAX_ARGUMENTS + 1];
    let first = arguments.next();
    let mut text = None;
    let mut file = None;
    positional[0] = match first.map(CStr::to_bytes) {
        Some(b"-c") => {
            let command = match arguments.next() {
                Some(end) if end.to_bytes() == b"--" => arguments.next(),
                command => command,
            };
            let Some(command) = command else {
                syscall::write_all(STANDARD_ERROR, b"eggsh: -c: option requires an argument\n");
                return USAGE;
            };
            text = Some(command.to_bytes());
            arguments.next().map_or(name, CStr::to_bytes)
        }
        Some(path) => {
            file = first;
            path
        }
        None => name,
    };
    let mut count = 1;
    for argument in arguments {
        if count == positional.len() {
            syscall::write_all(STANDARD_ERROR, b"eggsh: too many arguments\n");
            return USAGE;
        }
        positional[count] = argument.to_bytes();
        count += 1;
    }
    set_signal_action(SIGPIPE, SIG_IGN);
    let interactive = text.is_none() && file.is_none() && is_terminal(0) && is_terminal(2);
    let group = if interactive { take_the_terminal() } else { 0 };
    let pid = call(number::GETPID, [0; 6]) as u32;
    let root = call(number::GETEUID, [0; 6]) == 0;
    let settings = if interactive {
        terminal_settings()
    } else {
        None
    };
    let mut linux = Linux {
        group,
        settings,
        editing: false,
        editor: Editor::new(),
        history: History::default(),
        history_file: HistoryFile {
            path: [0; PATH_CAPACITY],
            length: 0,
            loaded: false,
        },
    };
    if interactive {
        linux.edit_lines(true);
    }
    let mut shell = Shell::new(linux, pid, &positional[..count], interactive, root);
    for variable in startup.environment() {
        shell.import(variable.to_bytes());
    }
    shell.set_working_directory_variable();
    shell.greet();
    if let Some(text) = text {
        shell.run_text(text);
    } else if let Some(file) = file {
        let descriptor = match shell.system.open(file.to_bytes_with_nul()) {
            Ok(descriptor) => descriptor,
            Err(errno) => {
                for part in [
                    &b"eggsh: "[..],
                    file.to_bytes(),
                    b": ",
                    errno.message(),
                    b"\n",
                ] {
                    syscall::write_all(STANDARD_ERROR, part);
                }
                return COMMAND_NOT_RUN;
            }
        };
        shell.run_descriptor(descriptor, false);
    } else {
        shell.run_descriptor(TERMINAL, interactive);
    }
    shell.system.edit_lines(false);
    shell.exit_status()
}
