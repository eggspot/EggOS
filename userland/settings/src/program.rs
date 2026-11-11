//! The `settings` program's work with the system: the arguments, the facts it reads from the
//! kernel, the settings file it reads and replaces, the profile it sets through `/dev/window`,
//! and the window's event loop.

use crate::disk_system::LinuxDisks;
use alloc::boxed::Box;
use alloc::string::String;
use core::ffi::CStr;
use core::fmt::Write;
use eggos_app::files::Files;
use eggos_app::linux::LinuxFiles;
use eggos_components::command as component_command;
use eggos_components::index::Component;
use eggos_components::manager::{self as component_manager, Failure, Refusal, RemoveError};
use eggos_components::state as component_state;
use eggos_eggfs::CheckMode;
use eggos_files::storage;
use eggos_mcp::audit::calls::LogFiles;
use eggos_mcp::grant_files::{self, GrantFiles, SaveError};
use eggos_mcp::grant_prompt::{
    ANSWER_BYTES, PROMPT_OPTION, PromptAnswer, QUESTION_BYTES, offers_always, read_question,
};
use eggos_mcp::grants::{self, AgentId, Grant};
use eggos_mcp::pairing::{self, PairingFiles};
use eggos_net::status::{self as network_status, InterfaceStatus};
use eggos_runtime::errno::Errno;
use eggos_runtime::process::{self, ChildStack, Spawn};
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{call, number, write_all};
use eggos_settings::activity::read_activity;
use eggos_settings::agents_pane::GrantsRead;
use eggos_settings::command::{
    self, MAX_ARGUMENTS, PairingAsked, USAGE, USAGE_STATUS, WindowAsked,
};
use eggos_settings::components_pane::{ComponentJob, ComponentsRead};
use eggos_settings::facts::Facts;
use eggos_settings::first_run::{self, FirstRunRequest, FirstRunWindow};
use eggos_settings::grant_prompt::{self as prompt, GrantPrompt, allowed_always};
use eggos_settings::keys::APPEARANCE_THEME;
use eggos_settings::keys::{DisplayChange, InputChange, KeyboardChoice, KeysChange};
use eggos_settings::network_view::NetworkRead;
use eggos_settings::wifi_section;
use eggos_settings::settings_window::{
    ABOUT, CAPACITY, DATE_AND_TIME, DEFAULT_SIZE, KEYBOARD, MINIMUM_SIZE, NETWORK, Request,
    SetOutcome, SettingsWindow, TITLE,
};
use eggos_settings::task_manager::{
    self, ProcessFact, Request as TaskRequest, Snapshot, TaskManager, WindowFact,
};
use eggos_settings_file::clock_sync::{self, Record};
use eggos_settings_file::{
    self as settings_file, DIRECTORIES, DisplayAccessibility, InputPreferences, KeyAccessibility,
    LARGEST, PATH, TEMPORARY_PATH_CAPACITY, TIME_AUTOMATIC,
};
use eggos_toolkit::application::{self, Application, View};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::elements::{Action, Outcome, TreeWriter};
use eggos_toolkit::input::Input;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::{Theme, palette_index, theme_of};
use eggos_toolkit::tree::{PointerShape, Response, WidgetTree};
use eggos_toolkit::window::{
    FIXED_SIZE, Keyboard, PERSON_INPUT_ONLY, Window, WindowServer, now_ms,
};
use eggos_windows_registry::linux::LinuxHiveFiles;
use eggos_windows_registry::store::FileStore;

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
const APP_COMMAND_NAME: &[u8] = b"app";
const FILES_PROGRAM_NAME: &[u8] = b"files";
const EDITOR_PROGRAM_NAME: &[u8] = b"text-editor";
const TASK_MANAGER_PROGRAM_NAME: &[u8] = b"task-manager";
const DOWNLOADS_PROGRAM_NAME: &[u8] = b"downloads";
const APPS_OPTION: &[u8] = b"--apps";
const WINDOW_DEVICE: &str = "/dev/window";
const O_RDWR_CLOEXEC: usize = 0o2_000_002;
const WNOHANG: usize = 1;
const LIST_WINDOW: usize = 0xc060_770b;
const MANAGE_WINDOW: usize = 0x4010_770c;
const LIST_SIZE: usize = 96;
const CLOSE_ACTION: u32 = 4;
const TASK_LIST_CAPACITY: usize = 16 * 1024;
const TERMINAL_PROGRAM_NAME: &[u8] = b"terminal";
const EDIT_COMMAND_NAME: &[u8] = b"edit";
const DISK_COMMAND_NAME: &[u8] = b"disk";
const CLOCK_REALTIME: usize = 0;
const HARDWARE_CLOCK: &str = "/proc/driver/rtc";
const SAVED_WITHIN_S: u64 = 2;
const NANOSECONDS_PER_MS: u32 = 1_000_000;
const HOME_VARIABLE: &[u8] = b"HOME=";
const DEFAULT_HOME: &str = "/home/root";

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

const DEVICES_TEXT_CAPACITY: usize = 32 * 1024;
const INTERFACES_TEXT_CAPACITY: usize = 2048;

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
    let path = CStr::from_bytes_until_nul(&path.bytes).map_err(|_| Errno::EINVAL)?;
    write_file_with_mode(path, FILE_MODE, bytes)
}

fn write_file_with_mode(path: &CStr, mode: usize, bytes: &[u8]) -> Result<(), Errno> {
    let fd = syscall(
        number::OPENAT,
        [
            AT_FDCWD,
            path.as_ptr() as usize,
            O_WRONLY_CREATE_TRUNCATE,
            mode,
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
    save_values_through_a_temporary_file(&[(key, value)])
}

fn save_values_through_a_temporary_file(values: &[(&str, &str)]) -> Result<(), Errno> {
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
    let mut new_length = 0;
    for (index, &(key, value)) in values.iter().enumerate() {
        let from = match index {
            0 => old[..length].to_vec(),
            _ => new[..new_length].to_vec(),
        };
        new_length = settings_file::with_value(&from, key, value.as_bytes(), &mut new)
            .map_err(|_| Errno::E2BIG)?;
    }
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

const WIFI_SETTINGS_MODE: usize = 0o600;
const WIFI_TEXT_BYTES: usize = 16 * 1024;

fn wifi_status() -> wifi_section::Status {
    let mut text = alloc::vec![0u8; WIFI_TEXT_BYTES];
    let mut status = match read_file(wifi_section::STATUS_PATH, &mut text) {
        Ok(length) => wifi_section::parse(core::str::from_utf8(&text[..length]).unwrap_or("")),
        Err(_) => wifi_section::Status::default(),
    };
    let mut settings = [0u8; 512];
    status.chosen_hidden = read_file(wifi_section::SETTINGS_PATH, &mut settings)
        .ok()
        .and_then(|length| core::str::from_utf8(&settings[..length]).ok())
        .is_some_and(wifi_section::chosen_hidden_in);
    status
}

fn wifi_chosen() -> Option<alloc::string::String> {
    let mut text = [0u8; 512];
    let length = read_file(wifi_section::SETTINGS_PATH, &mut text).ok()?;
    wifi_section::chosen_in(core::str::from_utf8(&text[..length]).ok()?)
}

fn network_adapter() -> Option<InterfaceStatus> {
    let mut text = [0u8; INTERFACES_TEXT_CAPACITY];
    let length = read_file(network_status::INTERFACES_PATH, &mut text).ok()?;
    network_status::adapter(&text[..length])
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

fn windows_layer_installed() -> bool {
    LinuxFiles::new()
        .status_following(eggos_mcp::tools::WINDOWS_LAYER.as_bytes())
        .is_ok()
}

fn components_read() -> ComponentsRead {
    let mut files = LinuxFiles::new();
    let arch = eggos_app::linux::machine().name();
    let store = component_manager::find_store(&mut files, arch);
    let rows = component_command::rows(&mut files, store.as_ref());
    let runs_this_window = rows
        .iter()
        .filter(|row| row.state == "installed")
        .find(|row| component_manager::runs_this_program(&mut files, &row.component))
        .map(|row| row.component.name.clone());
    let recorded_edition = component_command::recorded_edition(&mut files);
    let now_s = Some(wall_clock().0).filter(|now| *now >= FIRST_PLAUSIBLE_CLOCK_S);
    ComponentsRead {
        store,
        rows,
        runs_this_window,
        recorded_edition,
        checked: component_state::last_check(&mut files),
        daily: component_state::daily(&mut files),
        job: component_state::job(&mut files),
        now_s,
        network_up: network_adapter().is_some_and(|adapter| adapter.link_up && adapter.subnet.is_some())
            || wifi_status().joined.is_some(),
    }
}

const FIRST_PLAUSIBLE_CLOCK_S: u64 = 1_577_836_800;
const COMPONENT_JOB_POLL_MS: u32 = 100;

fn start_component_job(job: &ComponentJob) -> Result<usize, Errno> {
    let null = CPath::new("/dev/null");
    let quiet = syscall(
        number::OPENAT,
        [AT_FDCWD, null.pointer(), O_RDWR_CLOEXEC, 0, 0, 0],
    );
    let streams = match quiet {
        Ok(fd) => [fd, fd, fd],
        Err(_) => [0, 1, 2],
    };
    let words: alloc::vec::Vec<alloc::vec::Vec<u8>> = core::iter::once(String::from("component"))
        .chain(job.words())
        .map(|word| {
            let mut bytes = word.into_bytes();
            bytes.push(0);
            bytes
        })
        .collect();
    let mut arguments: alloc::vec::Vec<*const u8> = words.iter().map(|word| word.as_ptr()).collect();
    arguments.push(core::ptr::null());
    let environment = [core::ptr::null()];
    let request = Spawn::new(c"/bin/component", &arguments, &environment, streams).in_own_session();
    let mut stack = alloc::boxed::Box::new(ChildStack::new());
    let started = process::spawn(&mut stack, &request);
    if let Ok(fd) = quiet {
        close(fd);
    }
    started
}

fn child_ended(child: usize) -> Option<bool> {
    let mut status = 0i32;
    let waited = syscall(
        number::WAIT4,
        [child, &raw mut status as usize, WNOHANG, 0, 0, 0],
    );
    match waited {
        Ok(0) | Err(Errno::EINTR) => None,
        Ok(_) => Some(status & 0x7f == 0 && (status >> 8) & 0xff == 0),
        Err(_) => Some(false),
    }
}

fn check_component_remove(name: &str) -> Result<Option<Component>, Refusal> {
    let mut files = LinuxFiles::new();
    let installed = component_manager::installed(&mut files);
    component_manager::check_remove(&mut files, &installed, name)
}

fn locked_failure(refusal: Refusal) -> Failure {
    match refusal {
        Refusal::NotAllowed => Failure::NotAllowed,
        _ => Failure::Errno(Errno::EBUSY),
    }
}

fn install_component(name: &str, cancelled: bool) -> Result<(), Failure> {
    let mut files = LinuxFiles::new();
    let arch = eggos_app::linux::machine().name();
    let store = component_manager::find_store(&mut files, arch).ok_or(Failure::Detached)?;
    let component = store
        .index
        .component(name)
        .cloned()
        .ok_or(Failure::Detached)?;
    let pid = syscall(number::GETPID, [0; 6]).unwrap_or(0) as u32;
    component_manager::lock(&mut files, &component.title, false, pid).map_err(locked_failure)?;
    let installed =
        component_manager::install(&mut files, &store, &component, &mut |_, _| !cancelled);
    component_manager::unlock(&mut files);
    if installed.is_ok() {
        eggos_app::linux::apps_changed();
    }
    installed
}

fn remove_component(name: &str) -> Result<(), RemoveError> {
    let mut files = LinuxFiles::new();
    let installed = component_manager::installed(&mut files);
    let title = installed
        .iter()
        .find(|component| component.name == name)
        .map_or_else(|| String::from(name), |component| component.title.clone());
    let pid = syscall(number::GETPID, [0; 6]).unwrap_or(0) as u32;
    let removed = component_manager::remove_checked(&mut files, &title, name, pid).map(|_| ());
    if matches!(
        removed,
        Ok(()) | Err(RemoveError::Failed(Failure::FilesLeft(_)))
    ) {
        eggos_app::linux::apps_changed();
    }
    removed
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
    let saved_layouts = settings_file::keyboard_layout_set(&file[..length]);
    let saved_layout =
        settings_file::keyboard_layout(&file[..length]).unwrap_or_else(|| saved_layouts.keeping(0));
    let keyboard = server.and_then(|server| server.keyboard().ok());
    let form = server.and_then(|server| server.form().ok());
    Facts {
        version: env!("CARGO_PKG_VERSION"),
        architecture: architecture(),
        cpus: cpus(),
        memory_mb: memory_mb(),
        uptime_s: uptime_s(),
        resolution: area.map_or((0, 0), |area| {
            (
                area.rect.right().max(0) as u32 * area.scale.max(1),
                area.rect.bottom().max(0) as u32 * area.scale.max(1),
            )
        }),
        resolution_choice: settings_file::display_resolution(&file[..length]).flatten(),
        resolution_changeable: display_changeable(),
        scale: settings_file::display_scale(&file[..length])
            .unwrap_or_else(|| area.map_or(1, |area| area.scale)),
        scale_in_force: area.map_or(1, |area| area.scale.max(1)),
        profile,
        theme: saved_theme(&file[..length], server),
        on_disk: home_is_on_a_disk(),
        now_s: wall_clock().0,
        hardware_clock: hardware_clock_s().is_some(),
        keyboard_layout: keyboard.map_or(saved_layout, |keyboard| keyboard.layout as usize),
        keyboard_layouts: saved_layouts,
        switch_key: settings_file::keyboard_switch_key(&file[..length]),
        keyboard: keyboard.is_none_or(|keyboard| keyboard.present),
        caps_lock: keyboard.is_some_and(|keyboard| keyboard.caps_lock),
        time_zone: settings_file::value(&file[..length], settings_file::TIME_ZONE)
            .and_then(|name| core::str::from_utf8(name).ok())
            .and_then(eggos_settings::facts::zone_index)
            .unwrap_or_else(eggos_settings::facts::utc_zone),
        time_automatic: time_automatic(&file[..length]),
        clock_sync: clock_sync_record(),
        input: settings_file::input_preferences(&file[..length]),
        keys: settings_file::key_accessibility(&file[..length]),
        display: saved_display(&file[..length], server),
        network_adapter: network_adapter(),
        form_setting: form.map_or(0, |(_, setting, _)| setting as usize),
        form_in_use: form.map_or(0, |(in_use, _, _)| in_use as usize),
        input_kinds: form.map_or(0, |(_, _, kinds)| kinds),
        on_screen_keyboard: settings_file::value(&file[..length], settings_file::KEYBOARD_ON_SCREEN)
            .and_then(|chosen| {
                settings_file::KEYBOARD_ON_SCREEN_MODES
                    .iter()
                    .position(|mode| mode.as_bytes() == chosen)
            })
            .unwrap_or(0),
        rotation: settings_file::value(&file[..length], settings_file::DISPLAY_ROTATION)
            .and_then(|chosen| {
                settings_file::DISPLAY_ROTATIONS
                    .iter()
                    .position(|rotation| rotation.as_bytes() == chosen)
            })
            .unwrap_or(0),
    }
}

fn session_settings(file: &mut [u8; LARGEST + 1]) -> usize {
    read_file(settings_file::SESSION_PATH, file).unwrap_or(0)
}

fn time_automatic(disk: &[u8]) -> bool {
    let mut session = [0u8; LARGEST + 1];
    let length = session_settings(&mut session);
    settings_file::time_automatic(disk, &session[..length])
}

fn clock_sync_record() -> Option<Record> {
    let mut file = [0u8; clock_sync::LARGEST + 1];
    let length = read_file(clock_sync::PATH, &mut file).ok()?;
    Record::parse(&file[..length])
}

fn save_session_value(key: &str, value: &str) -> Result<(), Errno> {
    let folder = CPath::new(clock_sync::FOLDER);
    match syscall(
        number::MKDIRAT,
        [AT_FDCWD, folder.pointer(), DIRECTORY_MODE, 0, 0, 0],
    ) {
        Ok(_) | Err(Errno::EEXIST) => {}
        Err(error) => return Err(error),
    }
    let mut old = [0u8; LARGEST + 1];
    let length = session_settings(&mut old);
    let mut new = [0u8; LARGEST];
    let new_length = settings_file::with_value(&old[..length], key, value.as_bytes(), &mut new)
        .map_err(|_| Errno::E2BIG)?;
    let process_id = syscall(number::GETPID, [0; 6])?;
    let mut temporary = Text::<64>::new();
    let _ = write!(temporary, "{}.{process_id}", settings_file::SESSION_PATH);
    write_file(temporary.as_str(), &new[..new_length])?;
    let (from, to) = (
        CPath::new(temporary.as_str()),
        CPath::new(settings_file::SESSION_PATH),
    );
    syscall(
        number::RENAMEAT2,
        [AT_FDCWD, from.pointer(), AT_FDCWD, to.pointer(), 0, 0],
    )
    .map(|_| ())
}

fn save_time_automatic(on: bool) -> Result<(), Errno> {
    let value = match on {
        true => "on",
        false => "off",
    };
    let mut session = [0u8; LARGEST + 1];
    let length = session_settings(&mut session);
    let in_session = settings_file::value(&session[..length], TIME_AUTOMATIC).is_some();
    if home_is_on_a_disk() {
        save_through_a_temporary_file(TIME_AUTOMATIC, value)?;
        if !in_session {
            return Ok(());
        }
    }
    save_session_value(TIME_AUTOMATIC, value)
}

fn start_clock_sync() -> Result<usize, Errno> {
    let null = CPath::new("/dev/null");
    let quiet = syscall(
        number::OPENAT,
        [AT_FDCWD, null.pointer(), O_RDWR_CLOEXEC, 0, 0, 0],
    );
    let streams = match quiet {
        Ok(fd) => [fd, fd, fd],
        Err(_) => [0, 1, 2],
    };
    let arguments = [
        c"date".as_ptr().cast::<u8>(),
        c"--sync".as_ptr().cast::<u8>(),
        c"--automatic".as_ptr().cast::<u8>(),
        core::ptr::null(),
    ];
    let environment = [core::ptr::null()];
    let request = Spawn::new(c"/bin/date", &arguments, &environment, streams).in_own_session();
    let mut stack = alloc::boxed::Box::new(ChildStack::new());
    let started = process::spawn(&mut stack, &request);
    if let Ok(fd) = quiet {
        close(fd);
    }
    started
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

fn save_display_scale(scale: u32) -> Result<(), Errno> {
    let value = match scale {
        2 => "2",
        _ => "1",
    };
    save_through_a_temporary_file(settings_file::DISPLAY_SCALE, value)
}

fn save_display_resolution(size: Option<(u32, u32)>) -> Result<(), Errno> {
    let mut text = Text::<16>::new();
    let _ = match size {
        Some((width, height)) => core::fmt::Write::write_fmt(&mut text, format_args!("{width}x{height}")),
        None => core::fmt::Write::write_str(&mut text, "auto"),
    };
    save_through_a_temporary_file(settings_file::DISPLAY_RESOLUTION, text.as_str())
}

fn display_changeable() -> bool {
    let mut text = [0u8; 128];
    let length = read_file("/proc/driver/display", &mut text).unwrap_or(0);
    core::str::from_utf8(&text[..length]).is_ok_and(|text| text.contains("changeable yes"))
}

fn save_time_zone(index: usize) -> Result<(), Errno> {
    let name = eggos_calendar::zone::ZONES
        .get(index)
        .map_or("UTC", |(name, _)| *name);
    save_through_a_temporary_file(settings_file::TIME_ZONE, name)
}

fn save_keyboard(choice: KeyboardChoice) -> Result<(), Errno> {
    let mut layouts = [0u8; settings_file::KEYBOARD_LAYOUT_SET_CAPACITY];
    let in_force = settings_file::KEYBOARD_LAYOUTS
        .get(choice.in_force)
        .map_or("us", |layout| layout.name);
    save_values_through_a_temporary_file(&[
        (
            settings_file::KEYBOARD_LAYOUT_SET,
            choice.layouts.write(&mut layouts),
        ),
        (settings_file::KEYBOARD_LAYOUT, in_force),
        (settings_file::KEYBOARD_SWITCH_KEY, choice.switch_key.name()),
    ])
}

const KEYBOARD_LAYOUT_CONTROL: &str = "/proc/sys/eggos/keyboard_layout";
const KEYBOARD_LAYOUTS_CONTROL: &str = "/proc/sys/eggos/keyboard_layouts";
const KEYBOARD_SWITCH_KEY_CONTROL: &str = "/proc/sys/eggos/keyboard_switch_key";

fn apply_keyboard(choice: KeyboardChoice, server: Option<&WindowServer>) -> Result<(), Errno> {
    let opened;
    let server = match server {
        Some(server) => Some(server),
        None => {
            opened = WindowServer::open();
            opened.as_ref().ok()
        }
    };
    if let Some(server) = server {
        let mut numbers = [0u8; 4];
        for (place, number) in numbers.iter_mut().zip(choice.layouts.numbers()) {
            *place = number as u8;
        }
        server.set_keyboard_layouts(
            choice.layouts.len() as u32,
            numbers,
            choice.switch_key.number() as u32,
        )?;
        return server.set_keyboard_layout(choice.in_force as u32);
    }
    let mut layouts = [0u8; settings_file::KEYBOARD_LAYOUT_SET_CAPACITY];
    let in_force = settings_file::KEYBOARD_LAYOUTS
        .get(choice.in_force)
        .map_or("us", |layout| layout.name);
    write_file(
        KEYBOARD_LAYOUTS_CONTROL,
        choice.layouts.write(&mut layouts).as_bytes(),
    )?;
    write_file(
        KEYBOARD_SWITCH_KEY_CONTROL,
        choice.switch_key.name().as_bytes(),
    )?;
    write_file(KEYBOARD_LAYOUT_CONTROL, in_force.as_bytes())
}

fn apply_input(preferences: InputPreferences, server: Option<&WindowServer>) -> Result<(), Errno> {
    let opened;
    let server = match server {
        Some(server) => Some(server),
        None => {
            opened = WindowServer::open();
            opened.as_ref().ok()
        }
    };
    match server {
        Some(server) => server.set_input_preferences([
            u32::from(preferences.left_handed),
            u32::from(preferences.natural_scrolling),
            preferences.pointer_speed,
            preferences.repeat_delay_ms,
            preferences.repeat_rate,
        ]),
        None => Ok(()),
    }
}

fn choose_keyboard(choice: KeyboardChoice, server: Option<&WindowServer>) {
    if let Err(error) = apply_keyboard(choice, server) {
        say_error(&[
            settings_file::KEYBOARD_LAYOUT.as_bytes(),
            b": not applied: ",
            error.message(),
        ]);
    }
    if let Err(error) = save_keyboard(choice) {
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

fn apply_theme(theme: Theme, server: Option<&WindowServer>) -> Result<(), Errno> {
    let opened;
    let server = match server {
        Some(server) => Some(server),
        None => {
            opened = WindowServer::open();
            opened.as_ref().ok()
        }
    };
    match server {
        Some(server) => {
            let high_contrast = server.palette().is_ok_and(|index| index >= HIGH_CONTRAST);
            server.set_palette(palette_index(theme, high_contrast))
        }
        None => Ok(()),
    }
}

const HIGH_CONTRAST: u32 = 2;

fn apply_display(
    theme: Theme,
    display: DisplayAccessibility,
    server: Option<&WindowServer>,
) -> Result<(), Errno> {
    let opened;
    let server = match server {
        Some(server) => Some(server),
        None => {
            opened = WindowServer::open();
            opened.as_ref().ok()
        }
    };
    match server {
        Some(server) => {
            server.set_palette(palette_index(theme, display.high_contrast))?;
            server.set_reduce_motion(display.reduce_motion)
        }
        None => Ok(()),
    }
}

fn saved_display(file: &[u8], server: Option<&WindowServer>) -> DisplayAccessibility {
    let saved = settings_file::display_accessibility(file);
    DisplayAccessibility {
        high_contrast: server
            .and_then(|server| server.palette().ok())
            .map_or(saved.high_contrast, |index| index >= HIGH_CONTRAST),
        reduce_motion: server
            .and_then(|server| server.reduce_motion().ok())
            .unwrap_or(saved.reduce_motion),
    }
}

fn choose_theme(theme: Theme, server: Option<&WindowServer>) {
    if let Err(error) = apply_theme(theme, server) {
        say_error(&[
            APPEARANCE_THEME.as_bytes(),
            b": not applied: ",
            error.message(),
        ]);
    }
    if let Err(error) = save_through_a_temporary_file(APPEARANCE_THEME, theme.name()) {
        say_error(&[PATH.as_bytes(), b": ", error.message()]);
    }
}

fn saved_theme(file: &[u8], server: Option<&WindowServer>) -> Theme {
    let saved = settings_file::value(file, APPEARANCE_THEME)
        .and_then(|value| core::str::from_utf8(value).ok())
        .and_then(Theme::named)
        .unwrap_or_default();
    server
        .and_then(|server| server.palette().ok())
        .map_or(saved, theme_of)
}

struct Machine {
    server: Option<WindowServer>,
    coded_errors: bool,
}

impl command::Machine for Machine {
    fn facts(&mut self) -> Facts {
        facts(self.server.as_ref())
    }

    fn save_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]> {
        save_through_a_temporary_file(APPEARANCE_THEME, theme.name()).map_err(Errno::message)
    }

    fn apply_theme(&mut self, theme: Theme) -> Result<(), &'static [u8]> {
        apply_theme(theme, self.server.as_ref()).map_err(Errno::message)
    }

    fn save_form(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let name = eggos_settings::facts::FORM_SETTINGS[index];
        save_through_a_temporary_file(settings_file::SHELL_FORM, name).map_err(Errno::message)
    }

    fn save_on_screen_keyboard(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let mode = settings_file::KEYBOARD_ON_SCREEN_MODES[index.min(2)];
        save_through_a_temporary_file(settings_file::KEYBOARD_ON_SCREEN, mode).map_err(Errno::message)
    }

    fn apply_on_screen_keyboard(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let opened;
        let server = match self.server.as_ref() {
            Some(server) => server,
            None => {
                opened = WindowServer::open().map_err(Errno::message)?;
                &opened
            }
        };
        server.set_on_screen_keyboard_mode(index as u32).map_err(Errno::message)
    }

    fn save_rotation(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let rotation = settings_file::DISPLAY_ROTATIONS[index.min(4)];
        save_through_a_temporary_file(settings_file::DISPLAY_ROTATION, rotation).map_err(Errno::message)
    }

    fn apply_rotation(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let opened;
        let server = match self.server.as_ref() {
            Some(server) => server,
            None => {
                opened = WindowServer::open().map_err(Errno::message)?;
                &opened
            }
        };
        server.set_rotation(index as u32).map_err(Errno::message)
    }

    fn apply_form(&mut self, index: usize) -> Result<(), &'static [u8]> {
        let opened;
        let server = match self.server.as_ref() {
            Some(server) => server,
            None => {
                opened = WindowServer::open().map_err(Errno::message)?;
                &opened
            }
        };
        server.set_form(index as u32).map_err(Errno::message)?;
        let (in_use, _, _) = server.form().map_err(Errno::message)?;
        let wanted = eggos_settings::facts::FORM_SETTINGS[index];
        let shown = eggos_settings::facts::FORMS_IN_USE
            .get(in_use as usize)
            .copied()
            .unwrap_or("console");
        match wanted == "auto" || wanted == shown {
            true => Ok(()),
            false => Err(b"saved, but that form does not fit this screen; shell.form_in_use says the form shown"),
        }
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

    fn save_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]> {
        save_keyboard(choice).map_err(Errno::message)
    }

    fn apply_keyboard(&mut self, choice: KeyboardChoice) -> Result<(), &'static [u8]> {
        apply_keyboard(choice, self.server.as_ref()).map_err(Errno::message)
    }

    fn save_time_zone(&mut self, index: usize) -> Result<(), &'static [u8]> {
        save_time_zone(index).map_err(Errno::message)
    }

    fn save_time_automatic(&mut self, on: bool) -> Result<(), &'static [u8]> {
        save_time_automatic(on).map_err(Errno::message)
    }

    fn start_clock_sync(&mut self) -> Result<(), &'static [u8]> {
        start_clock_sync().map(|_| ()).map_err(Errno::message)
    }

    fn save_display_scale(&mut self, scale: u32) -> Result<(), &'static [u8]> {
        save_display_scale(scale).map_err(Errno::message)
    }

    fn save_display_resolution(&mut self, size: Option<(u32, u32)>) -> Result<(), &'static [u8]> {
        save_display_resolution(size).map_err(Errno::message)
    }

    fn save_input(&mut self, change: InputChange) -> Result<(), &'static [u8]> {
        let (key, value) = change.saved();
        save_through_a_temporary_file(key, value.as_str()).map_err(Errno::message)
    }

    fn apply_input(&mut self, preferences: InputPreferences) -> Result<(), &'static [u8]> {
        apply_input(preferences, self.server.as_ref()).map_err(Errno::message)
    }

    fn save_keys(&mut self, change: KeysChange) -> Result<(), &'static [u8]> {
        let (key, value) = change.saved();
        save_through_a_temporary_file(key, value.as_str()).map_err(Errno::message)
    }

    fn apply_keys(&mut self, keys: KeyAccessibility) -> Result<(), &'static [u8]> {
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
                .set_key_accessibility(keys.sticky_keys, keys.slow_keys, keys.slow_keys_ms)
                .map_err(Errno::message),
            None => Ok(()),
        }
    }

    fn save_display(&mut self, change: DisplayChange) -> Result<(), &'static [u8]> {
        let (key, value) = change.saved();
        save_through_a_temporary_file(key, value.as_str()).map_err(Errno::message)
    }

    fn apply_display(
        &mut self,
        theme: Theme,
        display: DisplayAccessibility,
    ) -> Result<(), &'static [u8]> {
        apply_display(theme, display, self.server.as_ref()).map_err(Errno::message)
    }

    fn write_output(&mut self, bytes: &[u8]) {
        write_all(STANDARD_OUTPUT, bytes);
    }

    fn write_error(&mut self, bytes: &[u8]) {
        write_all(STANDARD_ERROR, bytes);
    }

    fn wants_coded_errors(&self) -> bool {
        self.coded_errors
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentWork {
    Install,
    Remove,
}

#[derive(Debug, Clone, Copy)]
enum DiskWork {
    Check(&'static str, CheckMode),
    Format(&'static str, Text<LABEL_BYTES>),
    Install(&'static str),
}

const LABEL_BYTES: usize = 64;
const COMMAND_LINE_BYTES: usize = 1024;

struct LinuxGrantFiles;

impl GrantFiles for LinuxGrantFiles {
    fn read_whole(&mut self, path: &CStr, out: &mut [u8]) -> Result<usize, Errno> {
        read_file(path.to_str().map_err(|_| Errno::EINVAL)?, out)
    }

    fn make_directory(&mut self, path: &CStr, mode: u32) -> Result<(), Errno> {
        syscall(
            number::MKDIRAT,
            [AT_FDCWD, path.as_ptr() as usize, mode as usize, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn write_new(&mut self, path: &CStr, mode: u32, bytes: &[u8]) -> Result<(), Errno> {
        write_file_with_mode(path, mode as usize, bytes)
    }

    fn sync(&mut self) {
        let _ = syscall(number::SYNC, [0; 6]);
    }

    fn rename(&mut self, from: &CStr, to: &CStr) -> Result<(), Errno> {
        syscall(
            number::RENAMEAT2,
            [
                AT_FDCWD,
                from.as_ptr() as usize,
                AT_FDCWD,
                to.as_ptr() as usize,
                0,
                0,
            ],
        )
        .map(|_| ())
    }
}

impl PairingFiles for LinuxGrantFiles {
    fn remove(&mut self, path: &CStr) -> Result<(), Errno> {
        syscall(
            number::UNLINKAT,
            [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0],
        )
        .map(|_| ())
    }

    fn random(&mut self, out: &mut [u8]) -> Result<(), Errno> {
        let mut filled = 0;
        while let Some(free) = out.get_mut(filled..).filter(|free| !free.is_empty()) {
            match syscall(
                number::GETRANDOM,
                [free.as_mut_ptr() as usize, free.len(), 0, 0, 0, 0],
            ) {
                Ok(count) => filled += count,
                Err(Errno::EINTR) => {}
                Err(errno) => return Err(errno),
            }
        }
        Ok(())
    }
}

struct LinuxLogFiles;

impl LogFiles for LinuxLogFiles {
    fn open_log(&mut self, path: &CStr) -> Result<usize, Errno> {
        syscall(
            number::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, O_RDONLY_CLOEXEC, 0, 0, 0],
        )
    }

    fn read_log(&mut self, descriptor: usize, out: &mut [u8]) -> Result<usize, Errno> {
        loop {
            match syscall(
                number::READ,
                [descriptor, out.as_mut_ptr() as usize, out.len(), 0, 0, 0],
            ) {
                Err(Errno::EINTR) => {}
                read => return read,
            }
        }
    }

    fn close_log(&mut self, descriptor: usize) {
        close(descriptor);
    }
}

fn grants_read(on_disk: bool) -> GrantsRead {
    let mut line = [0u8; COMMAND_LINE_BYTES];
    let boot = match read_file(grants::COMMAND_LINE_PATH, &mut line) {
        Ok(length) => grants::from_command_line(&line[..length]).0,
        Err(_) => grants::Grants::NONE,
    };
    GrantsRead {
        store: grant_files::load(&mut LinuxGrantFiles),
        boot,
        now: wall_clock().0,
        on_disk,
    }
}

fn write_grants(window: &SettingsWindow) -> Result<(), SaveError> {
    grant_files::save(&mut LinuxGrantFiles, window.grants_to_write())
}

fn network_read() -> NetworkRead {
    let files = &mut LinuxGrantFiles;
    NetworkRead {
        state: pairing::network_state(files),
        fingerprint: pairing::certificate_fingerprint(files),
        pending: pairing::pending(files),
        now: wall_clock().0,
    }
}

struct Settings {
    window: SettingsWindow,
    server: Option<WindowServer>,
    uptime_from_ms: u32,
    uptime_at_start: u64,
    unsaved_profile: Option<usize>,
    disk_work: Option<DiskWork>,
    component_work: Option<ComponentWork>,
    clock_sync_child: Option<usize>,
    component_job_child: Option<usize>,
    component_side_children: alloc::vec::Vec<usize>,
}

const SECOND_MS: u32 = 1000;

impl Application<CAPACITY> for Settings {
    fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        self.window.tree()
    }

    fn respond(&mut self, response: Response, now_ms: u32) {
        match self.window.respond(response, now_ms) {
            Some(Request::SetTheme(theme)) => choose_theme(theme, self.server.as_ref()),
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
                let choice = KeyboardChoice::of(self.window.facts()).choosing(index);
                choose_keyboard(choice, self.server.as_ref());
            }
            Some(Request::SetKeyboardLayouts { layouts, in_force }) => {
                let choice = KeyboardChoice {
                    layouts,
                    in_force,
                    switch_key: self.window.facts().switch_key,
                };
                choose_keyboard(choice, self.server.as_ref());
            }
            Some(Request::SetSwitchKey(switch_key)) => {
                let choice = KeyboardChoice::of(self.window.facts()).switching_with(switch_key);
                choose_keyboard(choice, self.server.as_ref());
            }
            Some(Request::SetTimeZone(index)) => {
                self.window.time_zone_saved(index, save_time_zone(index));
            }
            Some(Request::SetTimeAutomatic { on, sync_now }) => {
                let saved = save_time_automatic(on);
                let started_at = match (saved, sync_now) {
                    (Ok(()), true) => self.start_clock_sync(),
                    _ => None,
                };
                self.window.time_automatic_saved(on, saved, started_at);
            }
            Some(Request::SetDisplayScale(scale)) => {
                self.window
                    .display_scale_saved(scale, save_display_scale(scale));
            }
            Some(Request::SetOnScreenKeyboard(mode)) => {
                let name = settings_file::KEYBOARD_ON_SCREEN_MODES[mode.min(2)];
                if save_through_a_temporary_file(settings_file::KEYBOARD_ON_SCREEN, name).is_ok()
                    && let Some(server) = self.server.as_ref()
                {
                    let _ = server.set_on_screen_keyboard_mode(mode as u32);
                }
            }
            Some(Request::SetShellForm(setting)) => {
                let name = eggos_settings::facts::FORM_SETTINGS[setting.min(4)];
                let saved = save_through_a_temporary_file(settings_file::SHELL_FORM, name);
                let applied = match (saved, self.server.as_ref()) {
                    (Ok(()), Some(server)) => server.set_form(setting as u32),
                    (result, _) => result,
                };
                let in_use = self
                    .server
                    .as_ref()
                    .and_then(|server| server.form().ok())
                    .map_or(0, |(in_use, _, _)| in_use as usize);
                self.window.shell_form_saved(setting, in_use, applied);
            }
            Some(Request::SetDisplayResolution(size)) => {
                self.window
                    .display_resolution_saved(size, save_display_resolution(size));
            }
            Some(Request::ChangeKeys(change)) => {
                let (key, value) = change.saved();
                let saved = save_through_a_temporary_file(key, value.as_str());
                let applied = match (saved, self.server.as_ref()) {
                    (Ok(()), Some(server)) => {
                        let keys = change.applied_to(self.window.facts().keys);
                        server.set_key_accessibility(
                            keys.sticky_keys,
                            keys.slow_keys,
                            keys.slow_keys_ms,
                        )
                    }
                    _ => Ok(()),
                };
                self.window.keys_changed(change, saved, applied);
            }
            Some(Request::ChangeDisplay(change)) => {
                let (key, value) = change.saved();
                let saved = save_through_a_temporary_file(key, value.as_str());
                let applied = match saved {
                    Ok(()) => {
                        let facts = self.window.facts();
                        let display = change.applied_to(facts.display);
                        apply_display(facts.theme, display, self.server.as_ref())
                    }
                    Err(_) => Ok(()),
                };
                self.window.display_changed(change, saved, applied);
            }
            Some(Request::ChangeInput(change)) => {
                let (key, value) = change.saved();
                let saved = save_through_a_temporary_file(key, value.as_str());
                let applied = match saved {
                    Ok(()) => {
                        let input = change.applied_to(self.window.facts().input);
                        apply_input(input, self.server.as_ref())
                    }
                    Err(_) => Ok(()),
                };
                self.window.input_changed(change, saved, applied);
            }
            Some(Request::ReadDisks) => {
                self.window.set_boot_files(storage::has_boot_files(&mut LinuxDisks));
                self.window.show_disks(storage::list(&mut LinuxDisks));
            }
            Some(Request::CheckDisk(device)) => {
                self.disk_work = Some(DiskWork::Check(device, CheckMode::ReadOnly));
            }
            Some(Request::RepairDisk(device)) => {
                self.disk_work = Some(DiskWork::Check(device, CheckMode::Repair));
            }
            Some(Request::FormatDisk(device)) => {
                let label = Text::from(self.window.format_label());
                self.disk_work = Some(DiskWork::Format(device, label));
            }
            Some(Request::InstallDisk(device)) => {
                self.disk_work = Some(DiskWork::Install(device));
            }
            Some(Request::ReadGrants) => {
                let read = grants_read(self.window.facts().on_disk);
                self.window.show_grants(read);
            }
            Some(Request::WriteGrants) => {
                let written = write_grants(&self.window);
                let read = grants_read(self.window.facts().on_disk);
                self.window.grants_written(written, read);
            }
            Some(Request::ReadActivity) => {
                let query = self.window.activity_query();
                let read = read_activity(&mut LinuxLogFiles, &query, wall_clock().0);
                self.window.show_activity(read);
            }
            Some(Request::ReadNetwork) => self.window.show_network(network_read()),
            Some(Request::TurnNetwork(port)) => {
                let turned =
                    pairing::turn_network(&mut LinuxGrantFiles, port).map_err(|(_, errno)| errno);
                self.window.network_turned(turned, network_read());
            }
            Some(Request::ReadDevices) => {
                let mut text = alloc::vec![0u8; DEVICES_TEXT_CAPACITY];
                let read = read_file(eggos_device_list::DEVICES_FILE, &mut text);
                self.window.show_devices(read.map(|length| &text[..length]));
            }
            Some(Request::BeginPairing) => {
                if let Some((agent, name)) = self.window.to_pair() {
                    let outcome =
                        pairing::begin_pairing(&mut LinuxGrantFiles, agent, name, wall_clock().0);
                    let grants = grants_read(self.window.facts().on_disk);
                    self.window
                        .paired(agent, outcome, grants, network_read(), now_ms);
                }
            }
            Some(Request::ReadComponents) => self.window.show_components(components_read()),
            Some(Request::CheckComponentRemove) => {
                if let Some(name) = self.window.components_pane().to_check().map(String::from) {
                    let checked = check_component_remove(&name);
                    self.window.show_remove_check(checked, now_ms);
                }
            }
            Some(Request::InstallComponent) => self.component_work = Some(ComponentWork::Install),
            Some(Request::RemoveComponent) => self.component_work = Some(ComponentWork::Remove),
            Some(Request::StartComponentJob) => self.start_component_job(),
            Some(Request::WifiJoin { name, passphrase, hidden }) => {
                let marked = if hidden { "hidden=yes\n" } else { "" };
                let text = alloc::format!("ssid={}\npassphrase={}\n{marked}", name.as_str(), passphrase.as_str());
                let saved = write_file_with_mode(c"/var/lib/eggos/wifi.conf", WIFI_SETTINGS_MODE, text.as_bytes())
                    .map_err(|error| core::str::from_utf8(error.message()).unwrap_or("the file could not be written"));
                self.window.wifi_join_saved(saved, now_ms);
                self.window.show_wifi(wifi_status(), wifi_chosen());
            }
            Some(Request::WifiForget) => {
                let removed = syscall(number::UNLINKAT, [AT_FDCWD, c"/var/lib/eggos/wifi.conf".as_ptr() as usize, 0, 0, 0, 0]);
                if let Err(error) = removed {
                    self.window
                        .wifi_forget_failed(core::str::from_utf8(error.message()).unwrap_or("the file could not be removed"));
                }
                self.window.show_wifi(wifi_status(), wifi_chosen());
            }
            Some(Request::OpenComponents) => {}
            Some(Request::CopyToClipboard) => {
                if let Some(server) = self.server.as_ref() {
                    let _ = server.set_clipboard(self.window.windows_pane().to_copy());
                }
            }
            None => self.show_clock(now_ms),
        }
        if self.window.pane() == KEYBOARD {
            self.show_keyboard();
        }
    }

    fn person_view_only(&self) -> bool {
        self.window.pairing_counting_down()
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        if self.unsaved_profile.is_some()
            || self.disk_work.is_some()
            || self.component_work.is_some()
        {
            return Some(now_ms);
        }
        if self.window.pairing_counting_down() {
            let (_, nanoseconds) = wall_clock();
            return Some(now_ms.wrapping_add(SECOND_MS - nanoseconds / NANOSECONDS_PER_MS));
        }
        let pane_tick = match self.window.pane() {
            ABOUT | NETWORK => {
                let elapsed = now_ms.wrapping_sub(self.uptime_from_ms);
                Some(now_ms.wrapping_add(SECOND_MS - elapsed % SECOND_MS))
            }
            DATE_AND_TIME => {
                let (_, nanoseconds) = wall_clock();
                Some(now_ms.wrapping_add(SECOND_MS - nanoseconds / NANOSECONDS_PER_MS))
            }
            _ => None,
        };
        let reap_tick = self
            .clock_sync_child
            .or(self.component_side_children.first().copied())
            .map(|_| now_ms.wrapping_add(SECOND_MS));
        let job_tick = self
            .component_job_child
            .map(|_| now_ms.wrapping_add(COMPONENT_JOB_POLL_MS));
        job_tick.or(pane_tick).or(reap_tick)
    }

    fn tick(&mut self, now_ms: u32) {
        self.save_unsaved();
        self.do_disk_work(now_ms);
        self.do_component_work();
        self.follow_component_job();
        self.show_clock(now_ms);
        self.clock_sync_running();
        if self.window.pairing_counting_down() {
            self.window.show_network(network_read());
        }
    }

    fn focused(&mut self, _now_ms: u32) {
        self.show_windows_layer();
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
            let mut listed = [0u8; settings_file::KEYBOARD_LAYOUT_SET_CAPACITY + 1];
            let layouts = read_file(KEYBOARD_LAYOUTS_CONTROL, &mut listed)
                .ok()
                .and_then(|length| {
                    let text = &listed[..length.min(listed.len())];
                    let text = text.strip_suffix(b"\n").unwrap_or(text);
                    settings_file::KeyboardLayoutSet::parse(text).ok()
                });
            let facts = self.window.facts();
            match layouts {
                Some(layouts)
                    if layouts != facts.keyboard_layouts || layout != facts.keyboard_layout =>
                {
                    self.window.show_keyboard_layouts(layouts, layout);
                }
                _ if layout != facts.keyboard_layout => self.window.show_keyboard_layout(layout),
                _ => {}
            }
            self.window
                .show_keyboard(keyboard.present, keyboard.caps_lock);
        }
    }
}

impl Settings {
    fn open_pane(&mut self, pane: usize) {
        match self.window.open_pane(pane) {
            Some(Request::ReadDisks) => {
                self.window.set_boot_files(storage::has_boot_files(&mut LinuxDisks));
                self.window.show_disks(storage::list(&mut LinuxDisks));
            }
            Some(Request::ReadGrants) => {
                let read = grants_read(self.window.facts().on_disk);
                self.window.show_grants(read);
            }
            Some(Request::ReadDevices) => {
                let mut text = alloc::vec![0u8; DEVICES_TEXT_CAPACITY];
                let read = read_file(eggos_device_list::DEVICES_FILE, &mut text);
                self.window.show_devices(read.map(|length| &text[..length]));
            }
            Some(Request::ReadComponents) => self.window.show_components(components_read()),
            _ if pane == NETWORK => {
                self.window.show_network_adapter(network_adapter());
                self.window.show_wifi(wifi_status(), wifi_chosen());
            }
            _ => {}
        }
    }

    fn show_windows_layer(&mut self) {
        if let Some(Request::ReadComponents) =
            self.window.show_windows_layer(windows_layer_installed())
        {
            self.window.show_components(components_read());
        }
    }

    fn start_component_job(&mut self) {
        let Some(job) = self.window.take_component_job() else {
            return;
        };
        let started = start_component_job(&job);
        match (started, job.is_awaited()) {
            (Ok(child), true) => self.component_job_child = Some(child),
            (Ok(child), false) => self.component_side_children.push(child),
            (Err(_), true) => self.window.show_component_job_ended(false, components_read()),
            (Err(_), false) => {}
        }
    }

    fn follow_component_job(&mut self) {
        self.component_side_children.retain(|child| child_ended(*child).is_none());
        let Some(child) = self.component_job_child else {
            return;
        };
        match child_ended(child) {
            None => self.window.show_components(components_read()),
            Some(succeeded) => {
                self.component_job_child = None;
                self.window.show_component_job_ended(succeeded, components_read());
                self.show_windows_layer();
            }
        }
    }

    fn do_component_work(&mut self) {
        let Some(work) = self.component_work.take() else {
            return;
        };
        let pane = self.window.components_pane();
        match work {
            ComponentWork::Install => {
                let Some(name) = pane.to_install().map(String::from) else {
                    return;
                };
                let installed = install_component(&name, pane.cancelled());
                let next = self.window.show_installed(installed, components_read());
                if next == Some(Request::InstallComponent) {
                    self.component_work = Some(ComponentWork::Install);
                } else if let Some(edition) = self.window.take_finished_edition() {
                    component_command::record_edition(&mut LinuxFiles::new(), edition);
                    self.window.show_components(components_read());
                }
            }
            ComponentWork::Remove => {
                let Some(name) = pane.to_remove().map(String::from) else {
                    return;
                };
                let removed = remove_component(&name);
                self.window.show_removed(removed, components_read());
            }
        }
        self.show_windows_layer();
    }

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

    fn do_disk_work(&mut self, now_ms: u32) {
        let Some(work) = self.disk_work.take() else {
            return;
        };
        let mut disks = LinuxDisks;
        match work {
            DiskWork::Check(device, mode) => {
                let result = storage::check(&mut disks, device, mode);
                self.window.show_disks(storage::list(&mut disks));
                self.window.show_check(device, mode, result);
            }
            DiskWork::Format(device, label) => {
                let result = storage::format(&mut disks, device, label.as_str());
                self.window.show_disks(storage::list(&mut disks));
                self.window
                    .show_formatted(device, label.as_str(), result, now_ms);
            }
            DiskWork::Install(device) => {
                let result = storage::boot_files(&mut disks)
                    .and_then(|files| storage::install(&mut disks, device, &files))
                    .map(|_| ());
                self.window.show_disks(storage::list(&mut disks));
                self.window.show_installed(device, result, now_ms);
            }
        }
    }

    fn show_clock(&mut self, now_ms: u32) {
        match self.window.pane() {
            ABOUT => self.show_uptime(now_ms),
            DATE_AND_TIME => {
                self.window.show_time(wall_clock().0);
                if self.window.facts().time_automatic {
                    let running = self.clock_sync_running();
                    self.window
                        .show_clock_sync(clock_sync_record(), network_adapter(), running);
                }
            }
            NETWORK => {
                self.window.show_network_adapter(network_adapter());
                self.window.show_wifi(wifi_status(), wifi_chosen());
            }
            _ => {}
        }
    }

    fn start_clock_sync(&mut self) -> Option<i64> {
        let started_at = wall_clock().0 as i64;
        if self.clock_sync_running() {
            return Some(started_at);
        }
        self.clock_sync_child = Some(start_clock_sync().ok()?);
        Some(started_at)
    }

    fn clock_sync_running(&mut self) -> bool {
        let Some(child) = self.clock_sync_child else {
            return false;
        };
        let mut status = 0i32;
        let waited = syscall(
            number::WAIT4,
            [child, &raw mut status as usize, WNOHANG, 0, 0, 0],
        );
        if matches!(waited, Ok(0) | Err(Errno::EINTR)) {
            return true;
        }
        self.clock_sync_child = None;
        false
    }

    fn show_uptime(&mut self, now_ms: u32) {
        let elapsed = u64::from(now_ms.wrapping_sub(self.uptime_from_ms) / SECOND_MS);
        self.window.show_uptime(self.uptime_at_start + elapsed);
    }
}

const STANDARD_INPUT: usize = 0;

struct PromptWindow {
    prompt: GrantPrompt,
    agent: Option<AgentId>,
    class: Option<Grant>,
    destructive: bool,
}

impl PromptWindow {
    fn finish(&mut self) -> ! {
        let mut answer = self.prompt.answer().unwrap_or(PromptAnswer::Deny);
        if answer == PromptAnswer::Always {
            answer = match offers_always(self.class, self.destructive) {
                true if self.write_always() => PromptAnswer::Always,
                true => PromptAnswer::Once,
                false => PromptAnswer::Deny,
            };
        }
        let mut line = Text::<ANSWER_BYTES>::new();
        let _ = line.write_str(answer.word());
        let _ = line.write_str("\n");
        write_all(STANDARD_OUTPUT, line.as_str().as_bytes());
        eggos_runtime::syscall::exit(0)
    }

    fn write_always(&self) -> bool {
        let (Some(agent), Some(class)) = (self.agent, self.class) else {
            return false;
        };
        let Some(store) = allowed_always(grant_files::load(&mut LinuxGrantFiles), &agent, class)
        else {
            return false;
        };
        grant_files::save(&mut LinuxGrantFiles, &store).is_ok()
    }

    fn finish_if_answered(&mut self) {
        if self.prompt.answer().is_some() {
            self.finish();
        }
    }
}

impl View for PromptWindow {
    fn resize(&mut self, width: i32, height: i32) {
        self.prompt.tree().resize(width, height);
    }

    fn handle(&mut self, input: Input, time_ms: u32) -> bool {
        self.prompt.handle(input, time_ms);
        self.finish_if_answered();
        true
    }

    fn take_damage(&mut self) -> Damage {
        self.prompt.tree().take_damage()
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>) {
        self.prompt.tree().paint(canvas);
    }

    fn pointer_shape(&mut self) -> PointerShape {
        self.prompt.tree().pointer_shape()
    }

    fn deadline(&mut self, now_ms: u32) -> Option<u32> {
        Some(self.prompt.deadline(now_ms))
    }

    fn tick(&mut self, now_ms: u32) {
        self.prompt.tick(now_ms);
        self.finish_if_answered();
    }

    fn close_requested(&mut self, _time_ms: u32) -> bool {
        self.prompt.close_requested();
        self.finish()
    }

    fn describe(&mut self, tree: &mut TreeWriter<'_>) -> bool {
        self.prompt.tree().describe(tree);
        true
    }

    fn act(&mut self, element: u32, action: Action, text: &str, time_ms: u32) -> Outcome {
        let (outcome, response) = self.prompt.tree().act(element, action, text, time_ms);
        self.prompt.respond(response);
        self.finish_if_answered();
        outcome
    }
}

fn read_question_line(line: &mut [u8]) -> usize {
    let mut length = 0;
    while length < line.len() && !line[..length].contains(&b'\n') {
        let space = &mut line[length..];
        match syscall(
            number::READ,
            [
                STANDARD_INPUT,
                space.as_mut_ptr() as usize,
                space.len(),
                0,
                0,
                0,
            ],
        ) {
            Ok(0) => break,
            Ok(read) => length += read,
            Err(Errno::EINTR) => {}
            Err(_) => break,
        }
    }
    length
}

fn grant_prompt() -> i32 {
    let mut line = [0u8; QUESTION_BYTES];
    let length = read_question_line(&mut line);
    let Some(question) = read_question(&line[..length]) else {
        say_error(&[b"--grant-prompt: stdin holds no question; the answer is deny"]);
        write_all(STANDARD_OUTPUT, b"deny\n");
        return USAGE_STATUS;
    };
    let mut window = match Window::create_with_flags(
        prompt::TITLE,
        prompt::WIDTH,
        prompt::HEIGHT,
        (prompt::WIDTH, prompt::HEIGHT),
        FIXED_SIZE | PERSON_INPUT_ONLY,
    ) {
        Ok(window) => window,
        Err(error) => {
            say_error(&[b"cannot open the grant prompt: ", error.message()]);
            return 1;
        }
    };
    let mut id = [0u8; grants::MAX_AGENT_ID_BYTES];
    let mut view = PromptWindow {
        prompt: GrantPrompt::new(&question, now_ms()),
        agent: question
            .agent
            .decode_into(&mut id)
            .and_then(|id| AgentId::stored(id.as_bytes())),
        class: question.class,
        destructive: question.destructive,
    };
    if let Err(error) = application::run_view(&mut window, &mut view) {
        say_error(&[b"the grant prompt: ", error.message()]);
    }
    view.prompt.close_requested();
    view.finish()
}

fn open_window(pairing: Option<PairingAsked<'_>>, pane: Option<usize>, home: &str) -> i32 {
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
        disk_work: None,
        component_work: None,
        clock_sync_child: None,
        component_job_child: None,
        component_side_children: alloc::vec::Vec::new(),
    };
    settings
        .window
        .attach_registry(Box::new(FileStore::new(LinuxHiveFiles, home.as_bytes())));
    settings.show_windows_layer();
    if let Some(asked) = pairing {
        let grants = grants_read(settings.window.facts().on_disk);
        settings
            .window
            .open_pairing(asked.id, asked.name, grants, network_read(), now_ms());
    }
    if let Some(pane) = pane {
        settings.open_pane(pane);
    }
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

struct FirstRun {
    window: FirstRunWindow,
    server: Option<WindowServer>,
}

impl FirstRun {
    fn choose(&mut self, request: Request) {
        let result = match request {
            Request::SetProfile(index) => {
                let name = settings_file::PROFILES[index.min(settings_file::PROFILES.len() - 1)];
                save_through_a_temporary_file(settings_file::SHELL_PROFILE, name).and_then(|()| {
                    match self.server.as_ref() {
                        Some(server) => server.set_desktop_profile(index as u32),
                        None => Ok(()),
                    }
                })
            }
            Request::SetKeyboardLayout(index) => {
                let choice = KeyboardChoice::of(self.window.facts()).choosing(index);
                save_keyboard(choice).and_then(|()| apply_keyboard(choice, self.server.as_ref()))
            }
            Request::SetTimeZone(index) => save_time_zone(index),
            _ => return,
        };
        self.window.chosen(request, result);
    }
}

fn finish_first_run(request: FirstRunRequest) -> ! {
    if let FirstRunRequest::Finish {
        save_profile: Some(index),
    } = request
    {
        save_profile(index);
    }
    eggos_runtime::syscall::exit(0)
}

impl Application<CAPACITY> for FirstRun {
    fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        self.window.tree()
    }

    fn respond(&mut self, response: Response, now_ms: u32) {
        match self.window.respond(response, now_ms) {
            Some(FirstRunRequest::Choose(request)) => self.choose(request),
            Some(finish @ FirstRunRequest::Finish { .. }) => finish_first_run(finish),
            None => {}
        }
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        self.window
            .shows_the_clock()
            .then(|| now_ms.wrapping_add(SECOND_MS - now_ms % SECOND_MS))
    }

    fn tick(&mut self, _now_ms: u32) {
        self.window.show_now(wall_clock().0);
    }
}

fn open_first_run() -> i32 {
    let (width, height) = first_run::SIZE;
    let created =
        Window::create_with_flags(first_run::TITLE, width, height, (width, height), FIXED_SIZE);
    let Ok(mut window) = created else {
        say_error(&[b"no display"]);
        return 1;
    };
    if let Ok(area) = window.server().work_area() {
        let (x, y) = first_run::centred_in(area.rect.width, area.rect.height, area.scale);
        let _ = window.set_position(x, y);
    }
    let mut file = [0u8; LARGEST + 1];
    let length = saved_settings(&mut file);
    let profile_saved =
        settings_file::value(&file[..length], settings_file::SHELL_PROFILE).is_some();
    let mut first_run = FirstRun {
        window: FirstRunWindow::new(facts(Some(window.server())), profile_saved),
        server: WindowServer::open().ok(),
    };
    let served = application::run(&mut window, &mut first_run);
    let closing = first_run.window.close_requested();
    if let Err(error) = served {
        say_error(&[b"its window: ", error.message()]);
    }
    finish_first_run(closing)
}

struct TaskManagerProgram {
    window: TaskManager,
    device: Option<usize>,
}

fn word_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn task_snapshot(device: Option<usize>) -> Snapshot {
    let mut ps = alloc::vec![0u8; TASK_LIST_CAPACITY];
    let processes = read_file("/proc/ps", &mut ps).ok().map(|length| {
        ps[..length]
            .split(|&byte| byte == b'\n')
            .filter_map(|line| {
                let mut fields = line.splitn(7, |&byte| byte == b' ');
                let mut number = || {
                    core::str::from_utf8(fields.next()?)
                        .ok()?
                        .parse::<u64>()
                        .ok()
                };
                let pid = number()? as u32;
                let parent = number()? as u32;
                let _group = number()?;
                let _session = number()?;
                let state = *fields.next()?.first()?;
                let cpu_ms = core::str::from_utf8(fields.next()?)
                    .ok()?
                    .parse::<u64>()
                    .ok();
                let name = core::str::from_utf8(fields.next()?).unwrap_or("?");
                Some(ProcessFact {
                    pid,
                    parent,
                    state,
                    cpu_ms,
                    name: Text::from(name),
                })
            })
            .collect::<alloc::vec::Vec<_>>()
    });
    let windows = device.map(|device| {
        let mut windows = alloc::vec::Vec::new();
        for index in 0u32.. {
            let mut entry = [0u8; LIST_SIZE];
            entry[..4].copy_from_slice(&index.to_ne_bytes());
            if syscall(
                number::IOCTL,
                [device, LIST_WINDOW, entry.as_mut_ptr() as usize, 0, 0, 0],
            )
            .is_err()
            {
                break;
            }
            let length = (word_at(&entry, 28) as usize).min(LIST_SIZE - 32);
            windows.push(WindowFact {
                id: word_at(&entry, 0),
                client_pid: word_at(&entry, 24),
                title: Text::from(core::str::from_utf8(&entry[32..32 + length]).unwrap_or("")),
            });
        }
        windows
    });
    let mut meminfo = [0u8; 512];
    let memory = read_file("/proc/meminfo", &mut meminfo)
        .ok()
        .and_then(|length| {
            let field = |name: &[u8]| {
                meminfo[..length]
                    .split(|&byte| byte == b'\n')
                    .find(|line| line.starts_with(name))
                    .and_then(first_number)
            };
            Some((field(b"MemTotal:")? * 1024, field(b"MemFree:")? * 1024))
        });
    let mut uptime = [0u8; 64];
    let uptime_s = read_file("/proc/uptime", &mut uptime)
        .ok()
        .and_then(|length| first_number(&uptime[..length]));
    Snapshot {
        processes,
        windows,
        cpus: cpus(),
        memory,
        uptime_s,
        now_ms: u64::from(now_ms()),
    }
}

impl TaskManagerProgram {
    fn act(&mut self, request: Option<TaskRequest>) {
        match request {
            None => {}
            Some(TaskRequest::Refresh) => {
                let more = self.window.refreshed(task_snapshot(self.device));
                self.act(more);
            }
            Some(TaskRequest::CloseWindowsOf(pid)) => {
                for window in self.window.windows_of(pid) {
                    let mut request = [0u8; 16];
                    request[..4].copy_from_slice(&window.to_ne_bytes());
                    request[4..8].copy_from_slice(&CLOSE_ACTION.to_ne_bytes());
                    if let Some(device) = self.device {
                        let _ = syscall(
                            number::IOCTL,
                            [
                                device,
                                MANAGE_WINDOW,
                                request.as_mut_ptr() as usize,
                                0,
                                0,
                                0,
                            ],
                        );
                    }
                }
            }
            Some(TaskRequest::Signal { pid, signal }) => {
                let stopped = self.window.is_stopped(pid);
                let sent =
                    syscall(number::KILL, [pid as usize, signal as usize, 0, 0, 0, 0]).map(|_| ());
                if sent.is_ok() && stopped && signal == task_manager::TERMINATE {
                    let _ = syscall(
                        number::KILL,
                        [pid as usize, task_manager::CONTINUE as usize, 0, 0, 0, 0],
                    );
                }
                self.window.signal_sent(pid, sent);
            }
        }
    }
}

impl Application<{ task_manager::CAPACITY }> for TaskManagerProgram {
    fn tree(&mut self) -> &mut WidgetTree<{ task_manager::CAPACITY }> {
        self.window.tree()
    }

    fn respond(&mut self, response: Response, now_ms: u32) {
        let request = self.window.respond(response, u64::from(now_ms));
        self.act(request);
    }

    fn deadline(&self, now_ms: u32) -> Option<u32> {
        Some(now_ms.wrapping_add(task_manager::REFRESH_MS))
    }

    fn tick(&mut self, _now_ms: u32) {
        self.act(Some(TaskRequest::Refresh));
    }
}

fn run_task_manager(startup: &Startup) -> i32 {
    let apps_only = startup
        .arguments()
        .skip(1)
        .any(|argument| argument.to_bytes() == APPS_OPTION);
    let (width, height) = task_manager::DEFAULT_SIZE;
    let mut window = match Window::create(
        task_manager::TITLE,
        width,
        height,
        task_manager::MINIMUM_SIZE,
    ) {
        Ok(window) => window,
        Err(error) => {
            write_all(STANDARD_ERROR, b"task-manager: cannot open its window: ");
            write_all(STANDARD_ERROR, error.message());
            write_all(STANDARD_ERROR, b"\n");
            return 1;
        }
    };
    let device = syscall(
        number::OPENAT,
        [
            AT_FDCWD,
            CPath::new(WINDOW_DEVICE).pointer(),
            O_RDWR_CLOEXEC,
            0,
            0,
            0,
        ],
    )
    .ok();
    let own_pid = syscall(number::GETPID, [0; 6]).unwrap_or(0) as u32;
    let mut program = TaskManagerProgram {
        window: TaskManager::new(own_pid, apps_only),
        device,
    };
    program.act(Some(TaskRequest::Refresh));
    match application::run(&mut window, &mut program) {
        Ok(()) => 0,
        Err(error) => {
            write_all(STANDARD_ERROR, b"task-manager: its window: ");
            write_all(STANDARD_ERROR, error.message());
            write_all(STANDARD_ERROR, b"\n");
            1
        }
    }
}

/// The program: see the crate's documentation. Started as `app-window`, it is the
/// installer's window instead (D134); as `app` the `app` command (D134, D214); as `files` the
/// Files window (D135); as `text-editor` the Text Editor and as `edit` its command (D149); as
/// `disk` the `disk` command (D192); as `downloads` the Downloads window (D225).
pub fn run(startup: &Startup) -> i32 {
    let started_as = startup.arguments().next().map_or(&b""[..], CStr::to_bytes);
    match eggos_app::files::file_name(started_as) {
        WINDOW_PROGRAM_NAME => return eggos_app::window_program::run(startup),
        APP_COMMAND_NAME => return eggos_app::command::run(startup),
        FILES_PROGRAM_NAME => return eggos_files::program::run(startup),
        EDITOR_PROGRAM_NAME => return eggos_editor::program::run(startup),
        EDIT_COMMAND_NAME => return eggos_editor::command::run(startup),
        DISK_COMMAND_NAME => return crate::disk_system::run(startup),
        TERMINAL_PROGRAM_NAME => return eggos_terminal_window::program::run(startup),
        TASK_MANAGER_PROGRAM_NAME => return run_task_manager(startup),
        DOWNLOADS_PROGRAM_NAME => return eggos_downloads::program::run(startup),
        _ => {}
    }
    let mut options = startup.arguments().skip(1).map(CStr::to_bytes);
    if options.next() == Some(PROMPT_OPTION.as_bytes()) && options.next().is_none() {
        return grant_prompt();
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
    let home = startup
        .environment()
        .find_map(|variable| variable.to_bytes().strip_prefix(HOME_VARIABLE))
        .and_then(|home| core::str::from_utf8(home).ok())
        .filter(|home| !home.is_empty())
        .unwrap_or(DEFAULT_HOME);
    match command::window_asked(&arguments[..count]) {
        WindowAsked::Pairing(asked) => return open_window(Some(asked), None, home),
        WindowAsked::Pane(pane) => return open_window(None, Some(pane), home),
        WindowAsked::FirstRun => return open_first_run(),
        WindowAsked::Usage => {
            write_all(STANDARD_ERROR, USAGE.as_bytes());
            return USAGE_STATUS;
        }
        WindowAsked::Other => {}
    }
    match count {
        0 => open_window(None, None, home),
        _ => {
            let mut machine = Machine {
                server: WindowServer::open().ok(),
                coded_errors: startup
                    .environment()
                    .any(|variable| variable == eggos_commands::tools::CODED_ERRORS_ENVIRONMENT),
            };
            command::run(&mut machine, &arguments[..count])
        }
    }
}
