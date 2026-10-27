//! The `settings` program's work with the system: the arguments, the facts it reads from the
//! kernel, the settings file it reads and replaces, the profile it sets through `/dev/window`,
//! and the window's event loop.

use core::ffi::CStr;
use core::fmt::Write;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{call, number, write_all};
use eggos_settings::facts::Facts;
use eggos_settings::keys::{self, Change, Key, PROFILE_CHOICES, Refusal};
use eggos_settings::settings_window::{
    CAPACITY, DEFAULT_SIZE, MINIMUM_SIZE, Request, SettingsWindow, TITLE,
};
use eggos_settings_file::{self as settings_file, DIRECTORIES, LARGEST, PATH, TEMPORARY_PATH};
use eggos_toolkit::application::{self, Application};
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetTree};
use eggos_toolkit::window::{Device, Window, now_ms};

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
const USAGE: &[u8] = b"usage: settings [get <key> | set <key> <value> | list]\n";

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
    write_file(TEMPORARY_PATH, &new[..new_length])?;
    let (from, to) = (CPath::new(TEMPORARY_PATH), CPath::new(PATH));
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

fn facts(device: Option<&Device>) -> Facts {
    let mut file = [0u8; LARGEST + 1];
    let length = saved_settings(&mut file);
    let saved = settings_file::profile(&file[..length]).unwrap_or(DEFAULT_PROFILE);
    let profile = device
        .and_then(|device| device.desktop_profile().ok())
        .map_or(saved, |profile| profile as usize);
    let area = device.and_then(|device| device.work_area().ok());
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

fn apply_profile(index: usize, device: Option<&Device>) -> bool {
    let opened;
    let device = match device {
        Some(device) => Some(device),
        None => {
            opened = Device::open();
            opened.as_ref().ok()
        }
    };
    let applied = match device {
        Some(device) => device.set_desktop_profile(index as u32),
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

fn print_line(parts: &[&str]) {
    let mut line = Text::<256>::new();
    for part in parts {
        let _ = line.write_str(part);
    }
    let _ = line.write_str("\n");
    write_all(STANDARD_OUTPUT, line.as_str().as_bytes());
}

fn command(arguments: &[&str]) -> i32 {
    let device = Device::open().ok();
    match arguments {
        ["list"] => {
            let facts = facts(device.as_ref());
            for key in Key::ALL {
                print_line(&[key.name(), "=", key.value(&facts).as_str()]);
            }
            0
        }
        ["get", name] => match Key::named(name) {
            Some(key) => {
                print_line(&[key.value(&facts(device.as_ref())).as_str()]);
                0
            }
            None => {
                say_error(&[name.as_bytes(), b": no such setting"]);
                1
            }
        },
        ["set", name, value] => match keys::change(name, value) {
            Ok((_, Change::Profile(index))) => {
                let saved = save_profile(index);
                match apply_profile(index, device.as_ref()) && saved {
                    true => 0,
                    false => 1,
                }
            }
            Err(Refusal::NoSuchSetting) => {
                say_error(&[name.as_bytes(), b": no such setting"]);
                1
            }
            Err(Refusal::ReadOnly) => {
                say_error(&[name.as_bytes(), b": read-only"]);
                1
            }
            Err(Refusal::InvalidValue) => {
                say_error(&[
                    name.as_bytes(),
                    b": invalid value '",
                    value.as_bytes(),
                    b"' (",
                    PROFILE_CHOICES.as_bytes(),
                    b")",
                ]);
                2
            }
        },
        _ => {
            write_all(STANDARD_ERROR, USAGE);
            2
        }
    }
}

struct Settings {
    window: SettingsWindow,
    uptime_from_ms: u32,
    uptime_at_start: u64,
    unsaved_profile: Option<usize>,
}

const ABOUT_PANE: usize = 5;
const SECOND_MS: u32 = 1000;

impl Application<CAPACITY> for Settings {
    fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        self.window.tree()
    }

    fn respond(&mut self, response: Response, now_ms: u32) {
        match self.window.respond(response, now_ms) {
            Some(Request::SetProfile(index)) => {
                apply_profile(index, None);
                self.unsaved_profile = Some(index);
            }
            None => {
                if self.window.pane() == ABOUT_PANE {
                    self.show_uptime(now_ms);
                }
            }
        }
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        if self.unsaved_profile.is_some() {
            return Some(now_ms);
        }
        if self.window.pane() != ABOUT_PANE {
            return None;
        }
        let elapsed = now_ms.wrapping_sub(self.uptime_from_ms);
        Some(now_ms.wrapping_add(SECOND_MS - elapsed % SECOND_MS))
    }

    fn tick(&mut self, now_ms: u32) {
        if let Some(index) = self.unsaved_profile.take() {
            save_profile(index);
        }
        if self.window.pane() == ABOUT_PANE {
            self.show_uptime(now_ms);
        }
    }
}

impl Settings {
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
    let facts = facts(Some(window.device()));
    let mut settings = Settings {
        window: SettingsWindow::new(facts),
        uptime_from_ms: now_ms(),
        uptime_at_start: facts.uptime_s,
        unsaved_profile: None,
    };
    match application::run(&mut window, &mut settings) {
        Ok(()) => 0,
        Err(error) => {
            say_error(&[b"its window: ", error.message()]);
            1
        }
    }
}

/// The program: see the crate's documentation.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = [""; 4];
    let mut count = 0;
    for argument in startup.arguments().skip(1).map(CStr::to_bytes) {
        if count == arguments.len() {
            write_all(STANDARD_ERROR, USAGE);
            return 2;
        }
        let Ok(argument) = core::str::from_utf8(argument) else {
            write_all(STANDARD_ERROR, USAGE);
            return 2;
        };
        arguments[count] = argument;
        count += 1;
    }
    match count {
        0 => open_window(),
        _ => command(&arguments[..count]),
    }
}
