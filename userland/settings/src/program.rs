//! The `settings` program's work with the system: the arguments, the facts it reads from the
//! kernel, the settings file it reads and replaces, the profile it sets through `/dev/window`,
//! and the window's event loop.

use core::ffi::CStr;
use core::fmt::Write;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{call, number, write_all};
use eggos_settings::command::{self, MAX_ARGUMENTS, USAGE, USAGE_STATUS};
use eggos_settings::facts::Facts;
use eggos_settings::settings_window::{
    ABOUT, CAPACITY, DATE_AND_TIME, DEFAULT_SIZE, KEYBOARD, MINIMUM_SIZE, Request, SetOutcome,
    SettingsWindow, TITLE,
};
use eggos_settings_file::{
    self as settings_file, DIRECTORIES, LARGEST, PATH, TEMPORARY_PATH_CAPACITY,
};
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetTree};
use eggos_toolkit::window::{Keyboard, Window, WindowServer, now_ms};

const STANDARD_OUTPUT: usize = 1;
const STANDARD_ERROR: usize = 2;
const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY_CLOEXEC: usize = 0o2_000_000;
const O_WRONLY_CREATE_TRUNCATE: usize = 0o1 | 0o100 | 0o1000 | 0o2_000_000;
const FILE_MODE: usize = 0o644;
const DIRECTORY_MODE: usize = 0o755;
const STATX_SIZE: usize = 256;
const STATX_BASIC_STATS: usize = 0x7ff;
const STATX_DEVICE_MAJOR: usize = 136;
const UTSNAME_SIZE: usize = 390;
const UTSNAME_MACHINE: usize = 260;
const UTSNAME_FIELD: usize = 65;
const AFFINITY_BYTES: usize = 128;
const PATH_CAPACITY: usize = 128;
const DEFAULT_PROFILE: usize = 1;
const O_DIRECTORY: usize = 0o200_000;
const PS_CAPACITY: usize = 4096;
const STALE_CAPACITY: usize = 8;
const DIRECTORY_BUFFER: usize = 1024;
const DIRENT_LENGTH: usize = 16;
const DIRENT_NAME: usize = 19;
const WINDOW_PROGRAM_NAME: &[u8] = b"app-window";
const FILES_PROGRAM_NAME: &[u8] = b"files";
const EDITOR_PROGRAM_NAME: &[u8] = b"text-editor";
const TERMINAL_PROGRAM_NAME: &[u8] = b"terminal";
const EDIT_COMMAND_NAME: &[u8] = b"edit";
const DISK_COMMAND_NAME: &[u8] = b"disk";
const CLOCK_REALTIME: usize = 0;
const HARDWARE_CLOCK: &str = "/proc/driver/rtc";
const SAVED_WITHIN_S: u64 = 2;
const NANOSECONDS_PER_MS: u32 = 1_000_000;

fn syscall(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: every caller passes buffers it owns and borrows for the call, sized as the call
    // reads or writes them, and no call here maps, unmaps or changes memory the program uses.
    Errno::result(unsafe { call(number, arguments) })
}

struct CPath {
    bytes: [u8; PATH_CAPACITY],
}

impl CPath {
    fn new(path: &str) -> Self {
        let mut bytes = [0u8; PATH_CAPACITY];
        let length = path.len().min(PATH_CAPACITY - 1);
        bytes[..length].copy_from_slice(&path.as_bytes()[..length]);
        Self { bytes }
    }

    fn pointer(&self) -> usize {
        self.bytes.as_ptr() as usize
    }
}

fn close(fd: usize) {
    let _ = syscall(number::CLOSE, [fd, 0, 0, 0, 0, 0]);
}

fn read_file(path: &str, buffer: &mut [u8]) -> Result<usize, Errno> {
    let path = CPath::new(path);
    let fd = syscall(
        number::OPENAT,
        [AT_FDCWD, path.pointer(), O_RDONLY_CLOEXEC, 0, 0, 0],
    )?;
    let mut length = 0;
    let result = loop {
        if length == buffer.len() {
            break Ok(length);
        }
        let rest = &mut buffer[length..];
        match syscall(
            number::READ,
            [fd, rest.as_mut_ptr() as usize, rest.len(), 0, 0, 0],
        ) {
            Ok(0) => break Ok(length),
            Ok(read) => length += read,
            Err(Errno::EINTR) => {}
            Err(error) => break Err(error),
        }
    };
    close(fd);
    result
}

fn write_file(path: &str, bytes: &[u8]) -> Result<(), Errno> {
    let path = CPath::new(path);
    let fd = syscall(
        number::OPENAT,
        [
            AT_FDCWD,
            path.pointer(),
            O_WRONLY_CREATE_TRUNCATE,
            FILE_MODE,
            0,
            0,
        ],
    )?;
    let mut rest = bytes;
    let mut result = Ok(());
    while !rest.is_empty() {
        match syscall(
            number::WRITE,
            [fd, rest.as_ptr() as usize, rest.len(), 0, 0, 0],
        ) {
            Ok(0) => {
                result = Err(Errno::EIO);
                break;
            }
            Ok(written) => rest = &rest[written.min(rest.len())..],
            Err(Errno::EINTR) => {}
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    if result.is_ok() {
        result = syscall(number::FSYNC, [fd, 0, 0, 0, 0, 0]).map(|_| ());
    }
    close(fd);
    result
}

fn saved_settings(file: &mut [u8; LARGEST + 1]) -> usize {
    read_file(PATH, file).unwrap_or(0)
}

fn remove_stale_temporary_files(own: u32) {
    let directory = CPath::new(DIRECTORIES[2]);
    let Ok(fd) = syscall(
        number::OPENAT,
        [
            AT_FDCWD,
            directory.pointer(),
            O_RDONLY_CLOEXEC | O_DIRECTORY,
            0,
            0,
            0,
        ],
    ) else {
        return;
    };
    let mut stale = [0u32; STALE_CAPACITY];
    let mut count = 0;
    let mut entries = [0u8; DIRECTORY_BUFFER];
    while let Ok(read) = syscall(
        number::GETDENTS64,
        [fd, entries.as_mut_ptr() as usize, entries.len(), 0, 0, 0],
    ) {
        if read == 0 || count == stale.len() {
            break;
        }
        let mut offset = 0;
        while offset + DIRENT_NAME < read.min(entries.len()) {
            let length = usize::from(u16::from_ne_bytes([
                entries[offset + DIRENT_LENGTH],
                entries[offset + DIRENT_LENGTH + 1],
            ]));
            if length <= DIRENT_NAME || offset + length > read {
                break;
            }
            let raw = &entries[offset + DIRENT_NAME..offset + length];
            let name = &raw[..raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len())];
            if let Some(owner) = settings_file::temporary_owner(name)
                && owner != own
                && count < stale.len()
            {
                stale[count] = owner;
                count += 1;
            }
            offset += length;
        }
    }
    close(fd);
    let mut ps = [0u8; PS_CAPACITY];
    let running = match read_file("/proc/ps", &mut ps) {
        Ok(length) if length < ps.len() => &ps[..length],
        _ => return,
    };
    for owner in stale[..count]
        .iter()
        .filter(|&&owner| !settings_file::is_listed(running, owner))
    {
        let mut temporary = [0u8; TEMPORARY_PATH_CAPACITY];
        let path = CPath::new(settings_file::temporary_path(*owner, &mut temporary));
        let _ = syscall(number::UNLINKAT, [AT_FDCWD, path.pointer(), 0, 0, 0, 0]);
    }
}

fn save_through_a_temporary_file(key: &str, value: &str) -> Result<(), Errno> {
    for directory in DIRECTORIES {
        let directory = CPath::new(directory);
        match syscall(
            number::MKDIRAT,
            [AT_FDCWD, directory.pointer(), DIRECTORY_MODE, 0, 0, 0],
        ) {
            Ok(_) | Err(Errno::EEXIST) => {}
            Err(error) => return Err(error),
        }
    }
    let mut old = [0u8; LARGEST + 1];
    let length = saved_settings(&mut old);
    let mut new = [0u8; LARGEST];
    let new_length = settings_file::with_value(&old[..length], key, value.as_bytes(), &mut new)
        .map_err(|_| Errno::E2BIG)?;
    let process_id = syscall(number::GETPID, [0; 6])? as u32;
    remove_stale_temporary_files(process_id);
    let mut temporary = [0u8; TEMPORARY_PATH_CAPACITY];
    let temporary = settings_file::temporary_path(process_id, &mut temporary);
    write_file(temporary, &new[..new_length])?;
    let (from, to) = (CPath::new(temporary), CPath::new(PATH));
    syscall(
        number::RENAMEAT2,
        [AT_FDCWD, from.pointer(), AT_FDCWD, to.pointer(), 0, 0],
    )
    .map(|_| ())
}

fn first_number(text: &[u8]) -> Option<u64> {
    let start = text.iter().position(u8::is_ascii_digit)?;
    let digits = text[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit());
    Some(digits.fold(0u64, |value, digit| value * 10 + u64::from(digit - b'0')))
}

fn memory_mb() -> u64 {
    let mut meminfo = [0u8; 512];
    let length = read_file("/proc/meminfo", &mut meminfo).unwrap_or(0);
    let meminfo = &meminfo[..length];
    let line = meminfo
        .split(|&byte| byte == b'\n')
        .find(|line| line.starts_with(b"MemTotal:"));
    line.and_then(first_number)
        .map_or(0, |kib| (kib + 512) / 1024)
}

fn uptime_s() -> u64 {
    let mut uptime = [0u8; 64];
    let length = read_file("/proc/uptime", &mut uptime).unwrap_or(0);
    first_number(&uptime[..length]).unwrap_or(0)
}

fn wall_clock() -> (u64, u32) {
    let mut time = [0u64; 2];
    match syscall(
        number::CLOCK_GETTIME,
        [CLOCK_REALTIME, time.as_mut_ptr() as usize, 0, 0, 0, 0],
    ) {
        Ok(_) => (time[0], time[1] as u32),
        Err(_) => (0, 0),
    }
}

fn hardware_clock_s() -> Option<u64> {
    let mut text = [0u8; 256];
    let length = read_file(HARDWARE_CLOCK, &mut text).ok()?;
    let seconds = eggos_calendar::parse_proc_driver_rtc(&text[..length])?;
    u64::try_from(seconds).ok()
}

fn set_clock(seconds: u64) -> SetOutcome {
    let time = [seconds, 0u64];
    match syscall(
        number::CLOCK_SETTIME,
        [CLOCK_REALTIME, time.as_ptr() as usize, 0, 0, 0, 0],
    ) {
        Ok(_) => match hardware_clock_s() {
            Some(hardware) if hardware.abs_diff(seconds) <= SAVED_WITHIN_S => SetOutcome::Saved,
            _ => SetOutcome::NoHardwareClock,
        },
        Err(Errno::EPERM) => SetOutcome::NotAllowed,
        Err(_) => SetOutcome::Failed,
    }
}

fn cpus() -> u32 {
    let mut mask = [0u8; AFFINITY_BYTES];
    match syscall(
        number::SCHED_GETAFFINITY,
        [0, AFFINITY_BYTES, mask.as_mut_ptr() as usize, 0, 0, 0],
    ) {
        Ok(_) => mask
            .iter()
            .map(|byte| byte.count_ones())
            .sum::<u32>()
            .max(1),
        Err(_) => 1,
    }
}

fn architecture() -> Text<16> {
    let mut name = [0u8; UTSNAME_SIZE];
    if syscall(number::UNAME, [name.as_mut_ptr() as usize, 0, 0, 0, 0, 0]).is_err() {
        return Text::new();
    }
    let machine = &name[UTSNAME_MACHINE..UTSNAME_MACHINE + UTSNAME_FIELD];
    let end = machine
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(machine.len());
    Text::from(core::str::from_utf8(&machine[..end]).unwrap_or(""))
}

fn home_is_on_a_disk() -> bool {
    let mut record = [0u8; STATX_SIZE];
    let home = CPath::new("/home");
    let found = syscall(
        number::STATX,
        [
            AT_FDCWD,
            home.pointer(),
            0,
            STATX_BASIC_STATS,
            record.as_mut_ptr() as usize,
            0,
        ],
    );
    let major = u32::from_ne_bytes(
        record[STATX_DEVICE_MAJOR..STATX_DEVICE_MAJOR + 4]
            .try_into()
            .unwrap_or([0; 4]),
    );
    found.is_ok() && major != 0
}

fn facts(server: Option<&WindowServer>) -> Facts {
    let mut file = [0u8; LARGEST + 1];
    let length = saved_settings(&mut file);
    let saved = settings_file::profile(&file[..length]).unwrap_or(DEFAULT_PROFILE);
    let profile = server
        .and_then(|server| server.desktop_profile().ok())
        .map_or(saved, |profile| profile as usize);
    let area = server.and_then(|server| server.work_area().ok());
    let saved_layout = settings_file::keyboard_layout(&file[..length]).unwrap_or(0);
    let keyboard = server.and_then(|server| server.keyboard().ok());
    Facts {
        version: env!("CARGO_PKG_VERSION"),
        architecture: architecture(),
        cpus: cpus(),
        memory_mb: memory_mb(),
        uptime_s: uptime_s(),
        resolution: area.map_or((0, 0), |area| {
            (
                area.rect.right().max(0) as u32,
                area.rect.bottom().max(0) as u32,
            )
        }),
        scale: area.map_or(1, |area| area.scale),
        profile,
        on_disk: home_is_on_a_disk(),
        now_s: wall_clock().0,
        hardware_clock: hardware_clock_s().is_some(),
        keyboard_layout: keyboard.map_or(saved_layout, |keyboard| keyboard.layout as usize),
        keyboard: keyboard.is_none_or(|keyboard| keyboard.present),
        caps_lock: keyboard.is_some_and(|keyboard| keyboard.caps_lock),
    }
}

fn say_error(parts: &[&[u8]]) {
    let mut line = Text::<256>::new();
    let _ = line.write_str("settings: ");
    for part in parts {
        let _ = line.write_str(core::str::from_utf8(part).unwrap_or("?"));
    }
    let _ = line.write_str("\n");
    write_all(STANDARD_ERROR, line.as_str().as_bytes());
}

fn save_profile(index: usize) -> bool {
    let name = settings_file::PROFILES[index];
    let saved = save_through_a_temporary_file(settings_file::SHELL_PROFILE, name);
    if let Err(error) = saved {
        say_error(&[PATH.as_bytes(), b": ", error.message()]);
    }
    saved.is_ok()
}

fn save_keyboard_layout(index: usize) -> Result<(), Errno> {
    let name = settings_file::KEYBOARD_LAYOUTS[index].name;
    save_through_a_temporary_file(settings_file::KEYBOARD_LAYOUT, name)
}

fn apply_keyboard_layout(index: usize, server: Option<&WindowServer>) -> Result<(), Errno> {
    let opened;
    let server = match server {
        Some(server) => Some(server),
        None => {
            opened = WindowServer::open();
            opened.as_ref().ok()
        }
    };
    match server {
        Some(server) => server.set_keyboard_layout(index as u32),
        None => Ok(()),
    }
}

fn choose_keyboard_layout(index: usize, server: Option<&WindowServer>) {
    if let Err(error) = apply_keyboard_layout(index, server) {
        say_error(&[
            settings_file::KEYBOARD_LAYOUT.as_bytes(),
            b": not applied: ",
            error.message(),
        ]);
    }
    if let Err(error) = save_keyboard_layout(index) {
        say_error(&[PATH.as_bytes(), b": ", error.message()]);
    }
}

fn apply_profile(index: usize, server: Option<&WindowServer>) -> bool {
    let opened;
    let server = match server {
        Some(server) => Some(server),
        None => {
            opened = WindowServer::open();
            opened.as_ref().ok()
        }
    };
    let applied = match server {
        Some(server) => server.set_desktop_profile(index as u32),
        None => Ok(()),
    };
    if let Err(error) = applied {
        say_error(&[
            settings_file::SHELL_PROFILE.as_bytes(),
            b": not applied: ",
            error.message(),
        ]);
    }
    applied.is_ok()
}

struct Machine {
    server: Option<WindowServer>,
}

impl command::Machine for Machine {
    fn facts(&mut self) -> Facts {
        facts(self.server.as_ref())
    }

    fn save_profile(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let name = settings_file::PROFILES[index];
        save_through_a_temporary_file(settings_file::SHELL_PROFILE, name).map_err(Errno::message)
    }

    fn apply_profile(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let opened;
        let server = match self.server.as_ref() {
            Some(server) => Some(server),
            None => {
                opened = WindowServer::open();
                opened.as_ref().ok()
            }
        };
        match server {
            Some(server) => server
                .set_desktop_profile(index as u32)
                .map_err(Errno::message),
            None => Ok(()),
        }
    }

    fn save_keyboard_layout(&mut self, index: usize) -> Result<(), &'static [u8]> {
        save_keyboard_layout(index).map_err(Errno::message)
    }

    fn apply_keyboard_layout(&mut self, index: usize) -> Result<(), &'static [u8]> {
        apply_keyboard_layout(index, self.server.as_ref()).map_err(Errno::message)
    }

    fn write_output(&mut self, bytes: &[u8]) {
        write_all(STANDARD_OUTPUT, bytes);
    }

    fn write_error(&mut self, bytes: &[u8]) {
        write_all(STANDARD_ERROR, bytes);
    }
}

struct Settings {
    window: SettingsWindow,
    server: Option<WindowServer>,
    uptime_from_ms: u32,
    uptime_at_start: u64,
    unsaved_profile: Option<usize>,
}

const SECOND_MS: u32 = 1000;

impl Application<CAPACITY> for Settings {
    fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        self.window.tree()
    }

    fn respond(&mut self, response: Response, now_ms: u32) {
        match self.window.respond(response, now_ms) {
            Some(Request::SetProfile(index)) => {
                apply_profile(index, self.server.as_ref());
                self.unsaved_profile = Some(index);
            }
            Some(Request::SetTime(seconds)) => {
                let outcome = set_clock(seconds);
                self.window.show_set_outcome(outcome, now_ms);
                self.window.show_time(wall_clock().0);
            }
            Some(Request::SetKeyboardLayout(index)) => {
                choose_keyboard_layout(index, self.server.as_ref());
            }
            None => self.show_clock(now_ms),
        }
        if self.window.pane() == KEYBOARD {
            self.show_keyboard();
        }
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        if self.unsaved_profile.is_some() {
            return Some(now_ms);
        }
        match self.window.pane() {
            ABOUT => {
                let elapsed = now_ms.wrapping_sub(self.uptime_from_ms);
                Some(now_ms.wrapping_add(SECOND_MS - elapsed % SECOND_MS))
            }
            DATE_AND_TIME => {
                let (_, nanoseconds) = wall_clock();
                Some(now_ms.wrapping_add(SECOND_MS - nanoseconds / NANOSECONDS_PER_MS))
            }
            _ => None,
        }
    }

    fn tick(&mut self, now_ms: u32) {
        self.save_unsaved();
        self.show_clock(now_ms);
    }

    fn focused(&mut self, _now_ms: u32) {
        if self.unsaved_profile.is_some() {
            return;
        }
        let current = self
            .server
            .as_ref()
            .and_then(|server| server.desktop_profile().ok())
            .map(|profile| profile as usize)
            .filter(|&profile| profile < settings_file::PROFILES.len());
        if let Some(profile) = current.filter(|&profile| profile != self.window.facts().profile) {
            self.window.show_profile(profile);
        }
        if let Some(keyboard) = self.keyboard() {
            let layout = keyboard.layout as usize;
            if layout != self.window.facts().keyboard_layout {
                self.window.show_keyboard_layout(layout);
            }
            self.window
                .show_keyboard(keyboard.present, keyboard.caps_lock);
        }
    }
}

impl Settings {
    fn keyboard(&self) -> Option<Keyboard> {
        self.server.as_ref()?.keyboard().ok()
    }

    fn show_keyboard(&mut self) {
        if let Some(keyboard) = self.keyboard() {
            self.window
                .show_keyboard(keyboard.present, keyboard.caps_lock);
        }
    }

    fn save_unsaved(&mut self) {
        if let Some(index) = self.unsaved_profile.take() {
            save_profile(index);
        }
    }

    fn show_clock(&mut self, now_ms: u32) {
        match self.window.pane() {
            ABOUT => self.show_uptime(now_ms),
            DATE_AND_TIME => self.window.show_time(wall_clock().0),
            _ => {}
        }
    }

    fn show_uptime(&mut self, now_ms: u32) {
        let elapsed = u64::from(now_ms.wrapping_sub(self.uptime_from_ms) / SECOND_MS);
        self.window.show_uptime(self.uptime_at_start + elapsed);
    }
}

fn open_window() -> i32 {
    let (width, height) = DEFAULT_SIZE;
    let mut window = match Window::create(TITLE, width, height, MINIMUM_SIZE) {
        Ok(window) => window,
        Err(error) => {
            say_error(&[b"cannot open its window: ", error.message()]);
            return 1;
        }
    };
    let facts = facts(Some(window.server()));
    let mut settings = Settings {
        window: SettingsWindow::new(facts),
        server: WindowServer::open().ok(),
        uptime_from_ms: now_ms(),
        uptime_at_start: facts.uptime_s,
        unsaved_profile: None,
    };
    let served = application::run(&mut window, &mut settings);
    settings.save_unsaved();
    match served {
        Ok(()) => 0,
        Err(error) => {
            say_error(&[b"its window: ", error.message()]);
            1
        }
    }
}

/// The program: see the crate's documentation. Started as `app-window`, it is the
/// installer's window instead (D134); as `files` the Files window (D135); as `text-editor` the
/// Text Editor and as `edit` its command (D149); as `disk` the `disk` command (D192).
pub fn run(startup: &Startup) -> i32 {
    let started_as = startup.arguments().next().map_or(&b""[..], CStr::to_bytes);
    match eggos_app::files::file_name(started_as) {
        WINDOW_PROGRAM_NAME => return eggos_app::window_program::run(startup),
        FILES_PROGRAM_NAME => return eggos_files::program::run(startup),
        EDITOR_PROGRAM_NAME => return eggos_editor::program::run(startup),
        EDIT_COMMAND_NAME => return eggos_editor::command::run(startup),
        DISK_COMMAND_NAME => return crate::disk_system::run(startup),
        TERMINAL_PROGRAM_NAME => return eggos_terminal_window::program::run(startup),
        _ => {}
    }
    let mut arguments = [""; MAX_ARGUMENTS];
    let mut count = 0;
    for argument in startup.arguments().skip(1).map(CStr::to_bytes) {
        if count == arguments.len() {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        }
        let Ok(argument) = core::str::from_utf8(argument) else {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        };
        arguments[count] = argument;
        count += 1;
    }
    match count {
        0 => open_window(),
        _ => {
            let mut machine = Machine {
                server: WindowServer::open().ok(),
            };
            command::run(&mut machine, &arguments[..count])
        }
    }
}
