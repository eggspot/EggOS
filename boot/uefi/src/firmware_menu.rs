use core::fmt::Write;
use core::ptr::{null, null_mut};

use crate::efi::{self, InputKey};
use crate::loader::{Firmware, print, read_file};
use crate::firmware_entry;
use crate::menu::{self, Action, Entry, Key, Kind, Menu, Text};
use crate::paths::{self, Path};

const TICK_US: usize = 50_000;
const TICKS_PER_SECOND: u32 = 20;
const MOST_OPTION_BYTES: usize = 1024;
const MOST_BOOT_ORDER_ENTRIES: usize = 64;
const NAME_UNITS: usize = 32;
const SETTINGS_FILE: &str = "BOOT.TXT";

fn name(text: &str) -> [u16; NAME_UNITS] {
    let mut out = [0u16; NAME_UNITS];
    for (slot, unit) in out.iter_mut().zip(text.encode_utf16().take(NAME_UNITS - 1)) {
        *slot = unit;
    }
    out
}

fn boot_option_name(number: u16) -> [u16; NAME_UNITS] {
    let mut text = Text::<16>::new();
    let _ = write!(text, "Boot{number:04X}");
    name(text.as_str())
}

fn runtime(firmware: &Firmware) -> Option<&efi::RuntimeServices> {
    // SAFETY: the firmware's system table, valid while boot services run.
    let services = unsafe { (*firmware.system).runtime_services };
    // SAFETY: the firmware's runtime services table, valid for as long as the machine runs.
    (!services.is_null()).then(|| unsafe { &*services })
}

fn variable(firmware: &Firmware, wanted: &str, buffer: &mut [u8]) -> Option<usize> {
    variable_named(firmware, &name(wanted), buffer)
}

fn variable_named(firmware: &Firmware, wanted: &[u16; NAME_UNITS], buffer: &mut [u8]) -> Option<usize> {
    let services = runtime(firmware)?;
    let mut size = buffer.len();
    // SAFETY: `wanted` is a NUL-ended UTF-16 name and `buffer` is writable for `size` bytes.
    let status = unsafe {
        (services.get_variable)(
            wanted.as_ptr(),
            &efi::GLOBAL_VARIABLE,
            null_mut(),
            &mut size,
            buffer.as_mut_ptr().cast(),
        )
    };
    (!efi::is_error(status)).then_some(size)
}

fn variable_exists(firmware: &Firmware, wanted: &[u16; NAME_UNITS]) -> bool {
    let Some(services) = runtime(firmware) else {
        return true;
    };
    let mut size = 0;
    // SAFETY: `wanted` is a NUL-ended UTF-16 name; a size of 0 lets the firmware write nothing.
    let status = unsafe {
        (services.get_variable)(wanted.as_ptr(), &efi::GLOBAL_VARIABLE, null_mut(), &mut size, null_mut())
    };
    status != efi::NOT_FOUND
}

fn set_variable(firmware: &Firmware, wanted: &str, bytes: &[u8]) -> bool {
    let Some(services) = runtime(firmware) else {
        return false;
    };
    let wanted = name(wanted);
    let attributes =
        efi::VARIABLE_NON_VOLATILE | efi::VARIABLE_BOOTSERVICE_ACCESS | efi::VARIABLE_RUNTIME_ACCESS;
    // SAFETY: `wanted` is a NUL-ended UTF-16 name and `bytes` readable for its length.
    let status = unsafe {
        (services.set_variable)(
            wanted.as_ptr(),
            &efi::GLOBAL_VARIABLE,
            attributes,
            bytes.len(),
            bytes.as_ptr().cast(),
        )
    };
    !efi::is_error(status)
}

fn restart(firmware: &Firmware) -> Result<(), &'static str> {
    let services = runtime(firmware).ok_or("the firmware cannot restart the machine")?;
    // SAFETY: a cold reset with no data; the firmware does not return.
    unsafe { (services.reset_system)(efi::RESET_COLD, efi::SUCCESS, 0, null()) }
}

fn add_firmware_entries(firmware: &Firmware, menu: &mut Menu) {
    let mut order = [0u8; MOST_BOOT_ORDER_ENTRIES * 2];
    if let Some(length) = variable(firmware, "BootOrder", &mut order) {
        for pair in order[..length.min(order.len())].chunks_exact(2) {
            let number = u16::from_le_bytes([pair[0], pair[1]]);
            let mut bytes = [0u8; MOST_OPTION_BYTES];
            let Some(length) = variable_named(firmware, &boot_option_name(number), &mut bytes) else {
                continue;
            };
            if let Some(option) = menu::load_option_of(&bytes[..length.min(bytes.len())]) {
                menu.add_load_option(number, &option);
            }
        }
    }
    let mut supported = [0u8; 8];
    if variable(firmware, "OsIndicationsSupported", &mut supported) == Some(8)
        && u64::from_le_bytes(supported) & efi::OS_INDICATIONS_BOOT_TO_FW_UI != 0
    {
        menu.add_firmware_settings();
    }
}

fn read_key(firmware: &Firmware) -> Option<Key> {
    // SAFETY: the firmware's system table, valid while boot services run.
    let input = unsafe { (*firmware.system).console_in };
    if input.is_null() {
        return None;
    }
    let mut key = InputKey::default();
    // SAFETY: `input` is the firmware's text input protocol and `key` is writable.
    let status = unsafe { ((*input).read_key_stroke)(input, &mut key) };
    if efi::is_error(status) {
        return None;
    }
    Some(match (key.scan_code, key.unicode_char) {
        (efi::SCAN_UP, _) => Key::Up,
        (efi::SCAN_DOWN, _) => Key::Down,
        (efi::SCAN_HOME, _) => Key::Home,
        (efi::SCAN_END, _) => Key::End,
        (efi::SCAN_ESCAPE, _) => Key::Escape,
        (_, 0x0d) => Key::Enter,
        (_, unit) => Key::Character(char::from_u32(u32::from(unit)).unwrap_or('\0')),
    })
}

fn stall(firmware: &Firmware, microseconds: usize) {
    // SAFETY: a firmware wait with no pointers of ours, while boot services run.
    unsafe { (firmware.services().stall)(microseconds) };
}

fn show_rows(menu: &Menu) {
    print("\n");
    for index in 0..menu.entries().count() {
        print(if index == menu.selected { "> " } else { "  " });
        print(menu.line(index).as_str());
        print("\n");
    }
}

fn show_hint(menu: &Menu) {
    let mut line = Text::<80>::new();
    let _ = write!(line, "\r{:<64}", menu.hint().as_str());
    print(line.as_str());
}

fn write_settings(root: *mut efi::File, own_path: &Path, menu: &Menu) {
    let Some(path) = paths::beside(own_path, SETTINGS_FILE) else {
        return;
    };
    let text = menu::settings_text(&menu.settings);
    let mut file: *mut efi::File = null_mut();
    let mode = efi::FILE_MODE_READ | efi::FILE_MODE_WRITE;
    // SAFETY: `root` is the volume's open root directory and `path` a NUL-ended UTF-16 path.
    let status = unsafe { ((*root).open)(root, &mut file, path.as_ptr(), mode, 0) };
    if !efi::is_error(status) && !file.is_null() {
        // SAFETY: `file` is open; deleting it also closes it.
        unsafe { ((*file).delete)(file) };
    }
    file = null_mut();
    // SAFETY: as above, now creating the file again.
    let status =
        unsafe { ((*root).open)(root, &mut file, path.as_ptr(), mode | efi::FILE_MODE_CREATE, 0) };
    if efi::is_error(status) || file.is_null() {
        return;
    }
    let mut length = text.as_bytes().len();
    // SAFETY: `file` is open for writing and `text` readable for `length` bytes.
    unsafe { ((*file).write)(file, &mut length, text.as_bytes().as_ptr().cast()) };
    // SAFETY: `file` is open.
    unsafe { ((*file).close)(file) };
}

fn start_entry(
    firmware: &Firmware,
    root: *mut efi::File,
    own_path: Option<&Path>,
    menu: &mut Menu,
    index: usize,
) -> Result<(), &'static str> {
    let before = menu.settings;
    let Some(entry) = menu.starting(index) else {
        return Ok(());
    };
    if menu.settings != before {
        if let Some(own_path) = own_path {
            write_settings(root, own_path, menu);
        }
    }
    start_kind(firmware, &entry)
}

fn start_kind(firmware: &Firmware, entry: &Entry) -> Result<(), &'static str> {
    match entry.kind {
        Kind::EggOs => Ok(()),
        Kind::Firmware(number) => {
            if !set_variable(firmware, "BootNext", &number.to_le_bytes()) {
                return Err("the firmware did not take the system to start next");
            }
            let mut line = Text::<80>::new();
            let _ = writeln!(line, "\nRestarting into {}...", entry.name.as_str());
            print(line.as_str());
            restart(firmware)
        }
        Kind::FirmwareSettings => {
            let mut indications = [0u8; 8];
            let current = match variable(firmware, "OsIndications", &mut indications) {
                Some(8) => u64::from_le_bytes(indications),
                _ => 0,
            };
            let wanted = current | efi::OS_INDICATIONS_BOOT_TO_FW_UI;
            if !set_variable(firmware, "OsIndications", &wanted.to_le_bytes()) {
                return Err("the firmware did not take the request for its settings");
            }
            print("\nRestarting into the firmware settings...\n");
            restart(firmware)
        }
    }
}

/// Shows the boot menu when the firmware lists another system to start (D651) and returns
/// once EggOS is the one to start; starting another system restarts the machine into it.
pub fn run(firmware: &Firmware, root: *mut efi::File, own_path: Option<&Path>) -> Result<(), &'static str> {
    let settings = own_path
        .and_then(|own| paths::beside(own, SETTINGS_FILE))
        .and_then(|path| read_file(firmware, root, &path).ok())
        .map_or_else(menu::Settings::new, menu::settings_of);
    let mut menu = Menu::new(settings);
    add_firmware_entries(firmware, &mut menu);
    menu.ready();
    if !menu.has_choice() {
        return Ok(());
    }
    if menu.settings.wait_s == 0 && read_key(firmware).is_none() {
        let default = menu.default;
        return start_entry(firmware, root, own_path, &mut menu, default);
    }
    show_rows(&menu);
    show_hint(&menu);
    let mut ticks = 0;
    loop {
        stall(firmware, TICK_US);
        let action = match read_key(firmware) {
            Some(key) => {
                let action = menu.press(key);
                show_rows(&menu);
                action
            }
            None => {
                ticks += 1;
                if ticks < TICKS_PER_SECOND {
                    continue;
                }
                ticks = 0;
                menu.tick()
            }
        };
        match action {
            Action::Wait => {}
            Action::SaveSettings => {
                if let Some(own_path) = own_path {
                    write_settings(root, own_path, &menu);
                }
            }
            Action::Start(index) => {
                print("\n");
                return start_entry(firmware, root, own_path, &mut menu, index);
            }
        }
        show_hint(&menu);
    }
}

/// Writes EggOS's own `Boot####` entry for the loader's file and puts it first in `BootOrder`
/// (D652), unless an entry named EggOS already starts that file; a firmware that refuses the
/// write leaves the machine as it was.
pub fn add_own_entry(firmware: &Firmware, partition: &[u8], file: &[u8]) {
    let mut order = [0u8; MOST_BOOT_ORDER_ENTRIES * 2];
    let order_length = variable(firmware, "BootOrder", &mut order).unwrap_or(0).min(order.len());
    for pair in order[..order_length].chunks_exact(2) {
        let number = u16::from_le_bytes([pair[0], pair[1]]);
        let mut bytes = [0u8; MOST_OPTION_BYTES];
        let Some(length) = variable_named(firmware, &boot_option_name(number), &mut bytes) else {
            continue;
        };
        if menu::load_option_of(&bytes[..length.min(bytes.len())])
            .is_some_and(|option| firmware_entry::starts_file(&option, file))
        {
            return;
        }
    }
    let mut path = [0u8; MOST_OPTION_BYTES];
    let Some(path_length) = firmware_entry::file_device_path(partition, file, &mut path) else {
        return;
    };
    let mut option = [0u8; MOST_OPTION_BYTES];
    let Some(option_length) = firmware_entry::load_option(&path[..path_length], &mut option) else {
        return;
    };
    let free = firmware_entry::first_free(|number| variable_exists(firmware, &boot_option_name(number)));
    let Some(number) = free else {
        return;
    };
    let mut wanted = Text::<16>::new();
    let _ = write!(wanted, "Boot{number:04X}");
    if !set_variable(firmware, wanted.as_str(), &option[..option_length]) {
        return;
    }
    let mut new_order = [0u8; MOST_BOOT_ORDER_ENTRIES * 2 + 2];
    if let Some(length) = firmware_entry::order_with_first(&order[..order_length], number, &mut new_order) {
        set_variable(firmware, "BootOrder", &new_order[..length]);
    }
}
