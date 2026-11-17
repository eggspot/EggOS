//! The loader's boot menu (D650; installer dual boot v0.1 section 6): its entries, read from
//! the firmware's load options, its two settings in `BOOT.TXT`, and what each key and each
//! second of the wait does. The loader draws it and talks to the firmware; this module only
//! decides, so the host tests every rule.

use core::fmt::Write;

pub const MAX_ENTRIES: usize = 9;
pub const NAME_BYTES: usize = 40;
pub const ID_BYTES: usize = 16;
pub const WAIT_STEPS: [u8; 5] = [0, 3, 5, 10, 30];
pub const DEFAULT_WAIT_S: u8 = 5;
pub const MOST_WAIT_S: u8 = 60;
pub const LOAD_OPTION_ACTIVE: u32 = 1;
pub const OUR_DESCRIPTION: &str = "EggOS";
pub const SETTINGS_BYTES: usize = 256;

const MEDIA: u8 = 4;
const HARD_DRIVE: u8 = 1;
const FILE_PATH: u8 = 4;
const END: u8 = 0x7f;
const DEFAULT_KEY: &str = "boot.default";
const WAIT_KEY: &str = "boot.menu_wait_s";
const LAST_KEY: &str = "boot.last";
const HIGH_CONTRAST_KEY: &str = "accessibility.high_contrast";

/// A short ASCII text of fixed room, written with `write!`, cut when full.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Text<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    pub fn of(text: &str) -> Self {
        let mut out = Self::new();
        let _ = out.write_str(text);
        out
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for byte in text.bytes() {
            if self.length == N {
                break;
            }
            self.bytes[self.length] = if byte.is_ascii() && !byte.is_ascii_control() {
                byte
            } else {
                b'?'
            };
            self.length += 1;
        }
        Ok(())
    }
}

pub type Id = Text<ID_BYTES>;
pub type Name = Text<NAME_BYTES>;

/// Which entry starts when the wait ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultEntry {
    EggOs,
    Last,
    Entry(Id),
}

/// The menu's settings, as `BOOT.TXT` keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub default: DefaultEntry,
    pub wait_s: u8,
    pub last: Option<Id>,
    pub high_contrast: bool,
}

impl Settings {
    pub const fn new() -> Self {
        Self {
            default: DefaultEntry::EggOs,
            wait_s: DEFAULT_WAIT_S,
            last: None,
            high_contrast: false,
        }
    }
}

/// The settings `BOOT.TXT`'s `key=value` lines hold; a missing, unknown or out-of-range one
/// keeps its default.
pub fn settings_of(text: &[u8]) -> Settings {
    let mut settings = Settings::new();
    let text = core::str::from_utf8(text).unwrap_or("");
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            DEFAULT_KEY => {
                settings.default = match value {
                    "eggos" => DefaultEntry::EggOs,
                    "last" => DefaultEntry::Last,
                    "" => continue,
                    id => DefaultEntry::Entry(Id::of(id)),
                }
            }
            WAIT_KEY => {
                if let Ok(wait) = value.parse::<u8>() {
                    if wait <= MOST_WAIT_S {
                        settings.wait_s = wait;
                    }
                }
            }
            LAST_KEY if !value.is_empty() => settings.last = Some(Id::of(value)),
            HIGH_CONTRAST_KEY => settings.high_contrast = value == "true",
            _ => {}
        }
    }
    settings
}

/// `BOOT.TXT`'s text for `settings`.
pub fn settings_text(settings: &Settings) -> Text<SETTINGS_BYTES> {
    let mut text = Text::new();
    let default = match &settings.default {
        DefaultEntry::EggOs => "eggos",
        DefaultEntry::Last => "last",
        DefaultEntry::Entry(id) => id.as_str(),
    };
    let _ = writeln!(text, "{DEFAULT_KEY}={default}");
    let _ = writeln!(text, "{WAIT_KEY}={}", settings.wait_s);
    if let Some(last) = &settings.last {
        let _ = writeln!(text, "{LAST_KEY}={}", last.as_str());
    }
    if settings.high_contrast {
        let _ = writeln!(text, "{HIGH_CONTRAST_KEY}=true");
    }
    text
}

/// A firmware load option (`Boot####`), as UEFI 2.10 section 3.1.3 lays it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadOption<'a> {
    pub attributes: u32,
    pub description: Name,
    pub device_path: &'a [u8],
}

/// The load option in a `Boot####` variable's bytes; `None` when they are cut short.
pub fn load_option_of(bytes: &[u8]) -> Option<LoadOption<'_>> {
    let attributes = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let path_bytes = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    let mut description = Name::new();
    let mut at = 6;
    let mut units = [0u16; 1];
    loop {
        let unit = u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?);
        at += 2;
        if unit == 0 {
            break;
        }
        units[0] = unit;
        for character in char::decode_utf16(units.iter().copied()) {
            let mut encoded = [0u8; 4];
            let _ = description.write_str(character.unwrap_or('?').encode_utf8(&mut encoded));
        }
    }
    let device_path = bytes.get(at..at.checked_add(path_bytes)?)?;
    Some(LoadOption {
        attributes,
        description,
        device_path,
    })
}

/// Whether a load option starts a file on a disk partition: a hard-drive media node followed
/// by a file-path node. Network and removable-device entries do not count.
pub fn starts_a_file_on_a_partition(device_path: &[u8]) -> bool {
    let mut partition = false;
    let mut at = 0;
    while let Some(header) = device_path.get(at..at + 4) {
        let length = u16::from_le_bytes([header[2], header[3]]) as usize;
        match (header[0], header[1]) {
            (END, _) => break,
            (MEDIA, HARD_DRIVE) => partition = true,
            (MEDIA, FILE_PATH) if partition => return true,
            _ => {}
        }
        if length < 4 {
            break;
        }
        at += length;
    }
    false
}

/// The base of an entry's id: its system in lower case (`windows`, `ubuntu`, `macos`), else its
/// description's first word, lower case.
pub fn system_id(description: &str) -> Id {
    let contains = |text: &str, word: &str| {
        text.as_bytes()
            .windows(word.len())
            .any(|window| window.eq_ignore_ascii_case(word.as_bytes()))
    };
    for (word, id) in [
        ("windows", "windows"),
        ("mac os", "macos"),
        ("macos", "macos"),
    ] {
        if contains(description, word) {
            return Id::of(id);
        }
    }
    let mut id = Id::new();
    let first = description.split_whitespace().next().unwrap_or("entry");
    for byte in first.bytes().filter(u8::is_ascii_alphanumeric) {
        let _ = id.write_char(byte.to_ascii_lowercase() as char);
    }
    if id.as_str().is_empty() {
        return Id::of("entry");
    }
    id
}

/// What starting an entry means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    EggOs,
    Firmware(u16),
    FirmwareSettings,
}

/// One row of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub id: Id,
    pub name: Name,
    pub kind: Kind,
}

/// What the loader does after a key or a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wait,
    Start(usize),
    SaveSettings,
}

/// A key the menu reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
    Character(char),
}

/// The menu: its entries, the row selected, the wait and the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    entries: [Option<Entry>; MAX_ENTRIES],
    count: usize,
    pub selected: usize,
    pub default: usize,
    pub remaining_s: Option<u8>,
    pub settings: Settings,
}

impl Menu {
    /// EggOS's menu with only its own entry.
    pub fn new(settings: Settings) -> Self {
        let mut menu = Self {
            entries: [None; MAX_ENTRIES],
            count: 0,
            selected: 0,
            default: 0,
            remaining_s: Some(settings.wait_s),
            settings,
        };
        menu.push(Kind::EggOs, OUR_DESCRIPTION, Id::of("eggos"));
        menu
    }

    fn push(&mut self, kind: Kind, name: &str, base: Id) {
        if self.count == MAX_ENTRIES {
            return;
        }
        let same_system = self
            .entries()
            .filter(|entry| entry.id.as_str().split('-').next() == Some(base.as_str()))
            .count();
        let id = match same_system {
            0 => base,
            same => {
                let mut id = base;
                let _ = write!(id, "-{}", same + 1);
                id
            }
        };
        self.entries[self.count] = Some(Entry {
            id,
            name: Name::of(name),
            kind,
        });
        self.count += 1;
    }

    /// Adds the firmware's load option `Boot{number:04X}` when it is active, starts a file on
    /// a partition and is not EggOS's own.
    pub fn add_load_option(&mut self, number: u16, option: &LoadOption<'_>) {
        let name = option.description.as_str();
        if option.attributes & LOAD_OPTION_ACTIVE == 0
            || name == OUR_DESCRIPTION
            || !starts_a_file_on_a_partition(option.device_path)
        {
            return;
        }
        self.push(Kind::Firmware(number), name, system_id(name));
    }

    /// Adds `Firmware settings` last, when the firmware can restart into its setup.
    pub fn add_firmware_settings(&mut self) {
        self.push(Kind::FirmwareSettings, "Firmware settings", Id::of("firmware"));
    }

    /// Its rows, in order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.entries[..self.count].iter().flatten()
    }

    /// Whether there is a choice to show: another system's entry besides EggOS's.
    pub fn has_choice(&self) -> bool {
        self.entries()
            .any(|entry| matches!(entry.kind, Kind::Firmware(_)))
    }

    /// Decides the default row from the settings and starts the wait; call once every entry
    /// is added.
    pub fn ready(&mut self) {
        let wanted = match self.settings.default {
            DefaultEntry::EggOs => None,
            DefaultEntry::Last => self.settings.last,
            DefaultEntry::Entry(id) => Some(id),
        };
        self.default = wanted
            .and_then(|id| self.entries().position(|entry| entry.id == id))
            .unwrap_or(0);
        self.selected = self.default;
        self.remaining_s = Some(self.settings.wait_s);
    }

    /// One second of the wait passed.
    pub fn tick(&mut self) -> Action {
        match self.remaining_s {
            Some(0) | Some(1) => {
                self.remaining_s = Some(0);
                Action::Start(self.default)
            }
            Some(left) => {
                self.remaining_s = Some(left - 1);
                Action::Wait
            }
            None => Action::Wait,
        }
    }

    /// A key was pressed: any key stops the wait for good.
    pub fn press(&mut self, key: Key) -> Action {
        self.remaining_s = None;
        let last = self.count.saturating_sub(1);
        match key {
            Key::Up => self.selected = if self.selected == 0 { last } else { self.selected - 1 },
            Key::Down => self.selected = if self.selected == last { 0 } else { self.selected + 1 },
            Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            Key::Enter => return Action::Start(self.selected),
            Key::Escape => {}
            Key::Character(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < self.count {
                    return Action::Start(index);
                }
            }
            Key::Character('d' | 'D') => {
                if let Some(entry) = self.entries().nth(self.selected) {
                    self.settings.default = match entry.kind {
                        Kind::EggOs => DefaultEntry::EggOs,
                        _ => DefaultEntry::Entry(entry.id),
                    };
                    self.default = self.selected;
                    return Action::SaveSettings;
                }
            }
            Key::Character('t' | 'T') => {
                let at = WAIT_STEPS.iter().position(|step| *step > self.settings.wait_s);
                self.settings.wait_s = WAIT_STEPS[at.unwrap_or(0)];
                return Action::SaveSettings;
            }
            Key::Character(_) => {}
        }
        Action::Wait
    }

    /// Records the entry about to start, for `boot.default=last`.
    pub fn starting(&mut self, index: usize) -> Option<Entry> {
        let entry = *self.entries().nth(index)?;
        self.settings.last = Some(entry.id);
        Some(entry)
    }

    /// The hint under the rows: the countdown, or the keys once it stopped.
    pub fn hint(&self) -> Text<64> {
        let mut text = Text::new();
        match self.remaining_s {
            Some(left) => {
                let name = self
                    .entries()
                    .nth(self.default)
                    .map_or(OUR_DESCRIPTION, |entry| entry.name.as_str());
                let _ = write!(text, "Starts {name} in {left} s");
            }
            None => {
                let _ = write!(text, "Enter: start   D: default   T: wait {} s", self.settings.wait_s);
            }
        }
        text
    }

    /// Row `index`'s line without a screen: `1) EggOS (default)`.
    pub fn line(&self, index: usize) -> Text<64> {
        let mut text = Text::new();
        if let Some(entry) = self.entries().nth(index) {
            let _ = write!(text, "{}) {}", index + 1, entry.name.as_str());
            if index == self.default {
                let _ = text.write_str(" (default)");
            }
        }
        text
    }

    /// The prompt without a screen: `Starts 1 in 5 s. Press a number.`
    pub fn serial_prompt(&self) -> Text<64> {
        let mut text = Text::new();
        match self.remaining_s {
            Some(left) => {
                let _ = write!(text, "Starts {} in {left} s. Press a number.", self.default + 1);
            }
            None => {
                let _ = text.write_str("Press a number.");
            }
        }
        text
    }

    /// `boot.entries`: the ids in order, `eggos,windows,firmware`.
    pub fn ids(&self) -> Text<{ ID_BYTES * MAX_ENTRIES }> {
        let mut text = Text::new();
        for (at, entry) in self.entries().enumerate() {
            if at > 0 {
                let _ = text.write_char(',');
            }
            let _ = text.write_str(entry.id.as_str());
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option_bytes(attributes: u32, description: &str, path: &[u8]) -> Vec<u8> {
        let mut bytes = attributes.to_le_bytes().to_vec();
        bytes.extend_from_slice(&(path.len() as u16).to_le_bytes());
        for unit in description.encode_utf16().chain([0]) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(path);
        bytes
    }

    fn disk_path() -> Vec<u8> {
        let mut path = vec![MEDIA, HARD_DRIVE, 42, 0];
        path.extend_from_slice(&[0; 38]);
        path.extend_from_slice(&[MEDIA, FILE_PATH, 8, 0, b'\\', 0, 0, 0]);
        path.extend_from_slice(&[END, 0xff, 4, 0]);
        path
    }

    fn windows_and_ubuntu() -> Menu {
        let mut menu = Menu::new(Settings::new());
        let path = disk_path();
        let network: &[u8] = &[END, 0xff, 4, 0];
        for (number, name) in [(1u16, "Windows Boot Manager"), (2, "ubuntu"), (3, "EggOS"), (4, "UEFI PXEv4")] {
            let bytes = option_bytes(LOAD_OPTION_ACTIVE, name, if number == 4 { network } else { path.as_slice() });
            menu.add_load_option(number, &load_option_of(&bytes).unwrap());
        }
        menu.add_firmware_settings();
        menu.ready();
        menu
    }

    #[test]
    fn the_menu_lists_eggos_then_the_disk_entries_then_the_firmware() {
        let menu = windows_and_ubuntu();
        assert!(menu.has_choice());
        assert_eq!(menu.ids().as_str(), "eggos,windows,ubuntu,firmware");
        assert_eq!(menu.line(0).as_str(), "1) EggOS (default)");
        assert_eq!(menu.line(1).as_str(), "2) Windows Boot Manager");
        assert_eq!(menu.hint().as_str(), "Starts EggOS in 5 s");
        assert_eq!(menu.serial_prompt().as_str(), "Starts 1 in 5 s. Press a number.");
        assert!(!Menu::new(Settings::new()).has_choice());
    }

    #[test]
    fn the_wait_counts_down_and_any_key_stops_it_for_good() {
        let mut menu = windows_and_ubuntu();
        for _ in 0..4 {
            assert_eq!(menu.tick(), Action::Wait);
        }
        assert_eq!(menu.tick(), Action::Start(0));
        let mut menu = windows_and_ubuntu();
        assert_eq!(menu.press(Key::Escape), Action::Wait);
        assert_eq!(menu.tick(), Action::Wait);
        assert_eq!(menu.hint().as_str(), "Enter: start   D: default   T: wait 5 s");
        assert_eq!(menu.press(Key::Up), Action::Wait);
        assert_eq!(menu.selected, 3);
        assert_eq!(menu.press(Key::Down), Action::Wait);
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.press(Key::Character('3')), Action::Start(2));
        assert_eq!(menu.press(Key::Character('9')), Action::Wait);
    }

    #[test]
    fn d_and_t_change_the_settings_boot_txt_keeps() {
        let mut menu = windows_and_ubuntu();
        menu.press(Key::Down);
        assert_eq!(menu.press(Key::Character('d')), Action::SaveSettings);
        assert_eq!(menu.default, 1);
        assert_eq!(menu.press(Key::Character('T')), Action::SaveSettings);
        assert_eq!(menu.settings.wait_s, 10);
        let text = settings_text(&menu.settings);
        assert_eq!(text.as_str(), "boot.default=windows\nboot.menu_wait_s=10\n");
        assert_eq!(settings_of(text.as_bytes()), menu.settings);
        menu.settings.wait_s = 30;
        menu.press(Key::Character('t'));
        assert_eq!(menu.settings.wait_s, 0);
    }

    #[test]
    fn boot_txt_keeps_defaults_for_what_it_does_not_say_well() {
        let settings = settings_of(b"boot.menu_wait_s=99\nnonsense\nboot.default=last\nboot.last=ubuntu\n");
        assert_eq!(settings.wait_s, DEFAULT_WAIT_S);
        assert_eq!(settings.default, DefaultEntry::Last);
        let mut menu = Menu::new(settings);
        let path = disk_path();
        let bytes = option_bytes(LOAD_OPTION_ACTIVE, "ubuntu", &path);
        menu.add_load_option(7, &load_option_of(&bytes).unwrap());
        menu.ready();
        assert_eq!(menu.default, 1);
        assert_eq!(menu.starting(0).map(|entry| entry.kind), Some(Kind::EggOs));
        assert_eq!(menu.settings.last, Some(Id::of("eggos")));
    }

    #[test]
    fn a_second_system_of_the_same_kind_is_numbered_and_inactive_ones_are_left_out() {
        let mut menu = Menu::new(Settings::new());
        let path = disk_path();
        for (attributes, name) in [(LOAD_OPTION_ACTIVE, "ubuntu"), (LOAD_OPTION_ACTIVE, "Ubuntu 24.04"), (0, "debian")] {
            let bytes = option_bytes(attributes, name, &path);
            menu.add_load_option(1, &load_option_of(&bytes).unwrap());
        }
        assert_eq!(menu.ids().as_str(), "eggos,ubuntu,ubuntu-2");
        assert_eq!(system_id("Mac OS X").as_str(), "macos");
        assert_eq!(load_option_of(&[1, 0, 0, 0, 9, 0, b'A', 0]), None);
    }
}
