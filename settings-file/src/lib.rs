//! The system settings file (D132): where the choices made in Settings, or with `settings set`,
//! are kept, and how they are written. A small text file: the line `eggos-settings 1`, then one
//! `key=value` line per setting. Reading is tolerant: a file without that first line, of another
//! version, or longer than [`LARGEST`] holds nothing, so every setting takes its default; lines
//! that are not a setting are skipped; the last line of a key wins. Writing keeps the settings
//! it does not know, so programs of different versions do not erase each other's keys. Nothing
//! here allocates or panics; the kernel reads the file at boot with it, and the Settings app
//! reads and writes it.

#![cfg_attr(not(test), no_std)]

/// Where the file is: under `/home`, so on the persistent disk when one is attached (D131), in
/// the home of `root`, the account every process runs as until there are accounts.
pub const PATH: &str = "/home/root/.config/eggos/settings";

/// The directories [`PATH`] needs, outermost first.
pub const DIRECTORIES: [&str; 3] = [
    "/home/root",
    "/home/root/.config",
    "/home/root/.config/eggos",
];

/// The start of the name a new version is written under, before it is renamed over [`PATH`]:
/// [`temporary_path`] ends it with the writer's process id, so two writers never share one.
pub const TEMPORARY_PREFIX: &str = "/home/root/.config/eggos/settings.tmp.";

/// The longest name [`temporary_path`] writes.
pub const TEMPORARY_PATH_CAPACITY: usize = TEMPORARY_PREFIX.len() + 10;

/// The temporary file's name for the process `process_id`, written into `out`.
pub fn temporary_path(process_id: u32, out: &mut [u8; TEMPORARY_PATH_CAPACITY]) -> &str {
    let prefix = TEMPORARY_PREFIX.as_bytes();
    out[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0u8; 10];
    let mut count = 0;
    let mut rest = process_id;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        rest /= 10;
        count += 1;
        if rest == 0 {
            break;
        }
    }
    for (at, digit) in digits[..count].iter().rev().enumerate() {
        out[prefix.len() + at] = *digit;
    }
    core::str::from_utf8(&out[..prefix.len() + count]).unwrap_or(TEMPORARY_PREFIX)
}

/// The writer process whose temporary file is called `name` in [`PATH`]'s directory, if
/// `name` is one: [`temporary_path`]'s last component.
pub fn temporary_owner(name: &[u8]) -> Option<u32> {
    let prefix = &TEMPORARY_PREFIX.as_bytes()[DIRECTORIES[2].len() + 1..];
    decimal(name.strip_prefix(prefix)?)
}

/// Whether `/proc/ps`'s text `ps` (one line per process, its id first) lists process `id`.
pub fn is_listed(ps: &[u8], id: u32) -> bool {
    ps.split(|&byte| byte == b'\n').any(|line| {
        let first = line.split(|&byte| byte == b' ').next().unwrap_or(b"");
        decimal(first) == Some(id)
    })
}

fn decimal(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || digits.len() > 10 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    digits.iter().try_fold(0u32, |value, digit| {
        value.checked_mul(10)?.checked_add(u32::from(digit - b'0'))
    })
}

/// The file's first line: its format and version.
pub const HEADER: &[u8] = b"eggos-settings 1";

/// The longest file that is read; a longer one holds nothing.
pub const LARGEST: usize = 4096;

/// The desktop's layout profile: one of [`PROFILES`].
pub const SHELL_PROFILE: &str = "shell.profile";

/// The display's scale from the next boot: `1` or `2`.
pub const DISPLAY_SCALE: &str = "display.scale";

/// The values of [`SHELL_PROFILE`], each at the index that is its number in `/dev/window`'s
/// `GET_PROFILE` and `SET_PROFILE` (docs/windows.md).
pub const PROFILES: [&str; 3] = ["windows", "macos", "linux"];

/// The scale `display.scale` holds, 1 or 2, if the file holds one (D195): what the kernel
/// shows at from its next boot.
pub fn display_scale(file: &[u8]) -> Option<u32> {
    match value(file, DISPLAY_SCALE)? {
        b"1" => Some(1),
        b"2" => Some(2),
        _ => None,
    }
}

/// Which hand the mouse's buttons are for: `right` (the default) or `left`, which swaps the
/// left and right buttons (D195).
pub const MOUSE_BUTTONS: &str = "mouse.buttons";
/// Which way the wheel scrolls: `traditional` (the default) or `natural` (D195).
pub const MOUSE_SCROLLING: &str = "mouse.scrolling";
/// How far a mouse's move takes the pointer: 1 to 10, 5 the default (D195).
pub const MOUSE_SPEED: &str = "mouse.speed";
/// Milliseconds before a held key repeats: 150 to 2000, 400 the default (D195).
pub const KEYBOARD_REPEAT_DELAY: &str = "keyboard.repeat_delay_ms";
/// Repeats a second of a held key: 2 to 60, 25 the default (D195).
pub const KEYBOARD_REPEAT_RATE: &str = "keyboard.repeat_rate";

/// Whether a tapped modifier latches for the next key: `true` or `false` (D205).
pub const STICKY_KEYS: &str = "accessibility.sticky_keys";
/// Whether a key counts only once held a while: `true` or `false` (D205).
pub const SLOW_KEYS: &str = "accessibility.slow_keys";
/// How long a key must be held while slow keys are on: 100 to 1000 ms, 300 the default.
pub const SLOW_KEYS_MS: &str = "accessibility.slow_keys_ms";

/// How the keys are made easier to press (D205), as the settings file holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyAccessibility {
    pub sticky_keys: bool,
    pub slow_keys: bool,
    /// 100 to 1000.
    pub slow_keys_ms: u32,
}

impl KeyAccessibility {
    /// What EggOS starts with: both off, a 300 ms delay.
    pub const DEFAULT: Self = Self {
        sticky_keys: false,
        slow_keys: false,
        slow_keys_ms: 300,
    };
}

/// The key accessibility `file` holds.
pub fn key_accessibility(file: &[u8]) -> KeyAccessibility {
    let on = |key: &str| value(file, key) == Some(&b"true"[..]);
    KeyAccessibility {
        sticky_keys: on(STICKY_KEYS),
        slow_keys: on(SLOW_KEYS),
        slow_keys_ms: number_in(file, SLOW_KEYS_MS, 100, 1000)
            .unwrap_or(KeyAccessibility::DEFAULT.slow_keys_ms),
    }
}

/// Whether every colour token takes its high-contrast value: `true` or `false` (D223).
pub const HIGH_CONTRAST: &str = "accessibility.high_contrast";
/// Whether nothing blinks: `true` or `false` (D223).
pub const REDUCE_MOTION: &str = "accessibility.reduce_motion";

/// How the screen is made easier to see (D223), as the settings file holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayAccessibility {
    pub high_contrast: bool,
    pub reduce_motion: bool,
}

/// The display accessibility `file` holds: each `false` unless the file says `true`.
pub fn display_accessibility(file: &[u8]) -> DisplayAccessibility {
    DisplayAccessibility {
        high_contrast: value(file, HIGH_CONTRAST) == Some(&b"true"[..]),
        reduce_motion: value(file, REDUCE_MOTION) == Some(&b"true"[..]),
    }
}

/// The mouse and key preferences the settings file holds (D195), each its default when absent
/// or out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPreferences {
    /// The buttons are swapped.
    pub left_handed: bool,
    /// The wheel scrolls the natural way.
    pub natural_scrolling: bool,
    /// 1 to 10.
    pub pointer_speed: u32,
    /// 150 to 2000.
    pub repeat_delay_ms: u32,
    /// 2 to 60.
    pub repeat_rate: u32,
}

impl InputPreferences {
    /// The preferences EggOS starts with.
    pub const DEFAULT: Self = Self {
        left_handed: false,
        natural_scrolling: false,
        pointer_speed: 5,
        repeat_delay_ms: 400,
        repeat_rate: 25,
    };
}

fn number_in(file: &[u8], key: &str, low: u32, high: u32) -> Option<u32> {
    let digits = value(file, key)?;
    if digits.is_empty() || digits.len() > 5 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = digits
        .iter()
        .fold(0u32, |number, &digit| number * 10 + u32::from(digit - b'0'));
    (low..=high).contains(&number).then_some(number)
}

/// The mouse and key preferences `file` holds.
pub fn input_preferences(file: &[u8]) -> InputPreferences {
    let default = InputPreferences::DEFAULT;
    InputPreferences {
        left_handed: value(file, MOUSE_BUTTONS) == Some(&b"left"[..]),
        natural_scrolling: value(file, MOUSE_SCROLLING) == Some(&b"natural"[..]),
        pointer_speed: number_in(file, MOUSE_SPEED, 1, 10).unwrap_or(default.pointer_speed),
        repeat_delay_ms: number_in(file, KEYBOARD_REPEAT_DELAY, 150, 2000)
            .unwrap_or(default.repeat_delay_ms),
        repeat_rate: number_in(file, KEYBOARD_REPEAT_RATE, 2, 60).unwrap_or(default.repeat_rate),
    }
}

/// The keyboard layout: the name of one of [`KEYBOARD_LAYOUTS`] (D185).
pub const KEYBOARD_LAYOUT: &str = "keyboard.layout";

/// The time zone times are shown in: an IANA zone name such as `Europe/Paris`, or `UTC`
/// (D194); `TZ` in a program's environment comes first.
pub const TIME_ZONE: &str = "time.zone";

/// A keyboard layout as Settings and `settings` name and describe it
/// (docs/ux/keyboard-v0.3.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardLayout {
    /// The value of [`KEYBOARD_LAYOUT`].
    pub name: &'static str,
    /// What the Keyboard pane's list shows.
    pub title: &'static str,
    /// Whether AltGr (or Option) types a third level of characters.
    pub third_level: bool,
    /// The spacing accents of its dead keys, in the pane's order, separated by spaces; empty
    /// for none.
    pub dead_keys: &'static str,
    /// The two capital letters the bar's layout indicator shows (D220).
    pub code: &'static str,
}

const fn entry(
    name: &'static str,
    title: &'static str,
    code: &'static str,
    third_level: bool,
    dead_keys: &'static str,
) -> KeyboardLayout {
    KeyboardLayout {
        name,
        title,
        third_level,
        dead_keys,
        code,
    }
}

/// The keyboard layouts, in the Keyboard pane's order, each at the index that is its number in
/// `/dev/window`'s `GET_KEYBOARD_LAYOUT` and `SET_KEYBOARD_LAYOUT` and in the kernel's table.
pub const KEYBOARD_LAYOUTS: [KeyboardLayout; 20] = [
    entry("us", "English (US)", "US", false, ""),
    entry("us-intl", "English (US, international)", "UI", true, "´ ` ^ ¨ ~"),
    entry("us-mac", "English (US, Mac)", "UM", true, "´ ` ^ ¨ ~"),
    entry("gb", "English (UK)", "GB", true, ""),
    entry("us-dvorak", "English (US, Dvorak)", "DV", false, ""),
    entry("us-colemak", "English (US, Colemak)", "CK", false, ""),
    entry("dk", "Danish", "DK", true, "´ ` ^ ¨ ~"),
    entry("fi", "Finnish", "FI", true, "´ ` ^ ¨ ~"),
    entry("fr", "French", "FR", true, "^ ¨"),
    entry("be", "French (Belgium)", "BE", true, "´ ` ^ ¨ ~"),
    entry("ca", "French (Canada)", "CA", true, "` ^ ¨ ~ ¸"),
    entry("de", "German", "DE", true, "´ ` ^"),
    entry("ch", "German (Switzerland)", "CH", true, "´ ` ^ ¨ ~"),
    entry("it", "Italian", "IT", true, ""),
    entry("no", "Norwegian", "NO", true, "´ ` ^ ¨ ~"),
    entry("br", "Portuguese (Brazil)", "BR", true, "´ ` ^ ¨ ~"),
    entry("pt", "Portuguese (Portugal)", "PT", true, "´ ` ^ ¨ ~"),
    entry("es", "Spanish", "ES", true, "´ ` ^ ¨"),
    entry("latam", "Spanish (Latin America)", "LA", true, "´ ` ^ ¨"),
    entry("se", "Swedish", "SE", true, "´ ` ^ ¨ ~"),
];

/// The number in [`KEYBOARD_LAYOUTS`] of the layout named `name`.
pub fn keyboard_layout_named(name: &[u8]) -> Option<usize> {
    KEYBOARD_LAYOUTS
        .iter()
        .position(|layout| layout.name.as_bytes() == name)
}

/// The number in [`KEYBOARD_LAYOUTS`] of the keyboard layout `file` holds, if it holds a known
/// one.
pub fn keyboard_layout(file: &[u8]) -> Option<usize> {
    keyboard_layout_named(value(file, KEYBOARD_LAYOUT)?)
}

/// The layouts the switch key cycles through: one to [`MAX_KEYBOARD_LAYOUTS`] names of
/// [`KEYBOARD_LAYOUTS`], in order, separated by commas without spaces (D220).
pub const KEYBOARD_LAYOUT_SET: &str = "keyboard.layouts";

/// The key that switches to the next layout of [`KEYBOARD_LAYOUT_SET`]: one of
/// [`SwitchKey::NAMES`] (D220).
pub const KEYBOARD_SWITCH_KEY: &str = "keyboard.switch_key";

/// The most layouts [`KEYBOARD_LAYOUT_SET`] holds.
pub const MAX_KEYBOARD_LAYOUTS: usize = 4;

/// The longest text [`KeyboardLayoutSet::write`] makes: four of the longest names and three
/// commas.
pub const KEYBOARD_LAYOUT_SET_CAPACITY: usize = 48;

/// One to [`MAX_KEYBOARD_LAYOUTS`] keyboard layouts, by their numbers in [`KEYBOARD_LAYOUTS`],
/// none twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardLayoutSet {
    layouts: [u8; MAX_KEYBOARD_LAYOUTS],
    count: u8,
}

/// Why a [`KEYBOARD_LAYOUT_SET`] value was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSetRefusal {
    /// It names no layout, or more than [`MAX_KEYBOARD_LAYOUTS`].
    TooMany,
    /// The layout of this number is listed twice.
    Repeated(usize),
    /// The bytes from `start` to `end` of the value name no layout.
    Unknown { start: usize, end: usize },
}

impl KeyboardLayoutSet {
    /// The US layout alone, the default.
    pub const DEFAULT: Self = Self::of_one(0);

    /// Layout `number` alone.
    pub const fn of_one(number: usize) -> Self {
        Self {
            layouts: [number as u8, 0, 0, 0],
            count: 1,
        }
    }

    /// The set of `numbers` in order, if they are one to four layouts and none twice.
    pub fn of(numbers: &[usize]) -> Option<Self> {
        if numbers.is_empty() || numbers.len() > MAX_KEYBOARD_LAYOUTS {
            return None;
        }
        let mut set = Self {
            layouts: [0; MAX_KEYBOARD_LAYOUTS],
            count: 0,
        };
        for &number in numbers {
            if number >= KEYBOARD_LAYOUTS.len() || set.contains(number) {
                return None;
            }
            set.layouts[usize::from(set.count)] = number as u8;
            set.count += 1;
        }
        Some(set)
    }

    /// The set a [`KEYBOARD_LAYOUT_SET`] value names.
    pub fn parse(value: &[u8]) -> Result<Self, LayoutSetRefusal> {
        let mut numbers = [0usize; MAX_KEYBOARD_LAYOUTS];
        let mut count = 0;
        let mut start = 0;
        for name in value.split(|&byte| byte == b',') {
            let end = start + name.len();
            let number = keyboard_layout_named(name).ok_or(LayoutSetRefusal::Unknown { start, end });
            start = end + 1;
            let number = number?;
            if numbers[..count.min(MAX_KEYBOARD_LAYOUTS)].contains(&number) {
                return Err(LayoutSetRefusal::Repeated(number));
            }
            if count == MAX_KEYBOARD_LAYOUTS {
                return Err(LayoutSetRefusal::TooMany);
            }
            numbers[count] = number;
            count += 1;
        }
        Self::of(&numbers[..count]).ok_or(LayoutSetRefusal::TooMany)
    }

    /// The layouts' numbers, in order.
    pub fn numbers(&self) -> impl Iterator<Item = usize> + '_ {
        self.layouts[..usize::from(self.count)]
            .iter()
            .map(|&number| usize::from(number))
    }

    /// How many layouts it holds.
    pub const fn len(&self) -> usize {
        self.count as usize
    }

    /// Whether it holds nothing: never, as every set holds a layout.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The layout at `position`, if there is one.
    pub fn get(&self, position: usize) -> Option<usize> {
        self.numbers().nth(position)
    }

    /// Where layout `number` is in the set, if it is there.
    pub fn position(&self, number: usize) -> Option<usize> {
        self.numbers().position(|listed| listed == number)
    }

    /// Whether layout `number` is in the set.
    pub fn contains(&self, number: usize) -> bool {
        self.position(number).is_some()
    }

    /// The set with layout `number` put in force while `in_force` is: unchanged when it is
    /// listed, appended when there is room, else in the place of `in_force` (D220).
    pub fn with_chosen(&self, number: usize, in_force: usize) -> Self {
        if self.contains(number) || number >= KEYBOARD_LAYOUTS.len() {
            return *self;
        }
        let mut set = *self;
        if set.len() < MAX_KEYBOARD_LAYOUTS {
            set.layouts[set.len()] = number as u8;
            set.count += 1;
            return set;
        }
        let place = set.position(in_force).unwrap_or(0);
        set.layouts[place] = number as u8;
        set
    }

    /// The layout in force once this set is chosen while `in_force` was: `in_force` when it is
    /// listed, else the first.
    pub fn keeping(&self, in_force: usize) -> usize {
        match self.contains(in_force) {
            true => in_force,
            false => usize::from(self.layouts[0]),
        }
    }

    /// The layout after `number` in the set, or before it when `backwards`, wrapping round;
    /// the first for a layout not listed.
    pub fn next_after(&self, number: usize, backwards: bool) -> usize {
        let count = self.len().max(1);
        let next = match (self.position(number), backwards) {
            (None, _) => 0,
            (Some(at), false) => (at + 1) % count,
            (Some(at), true) => (at + count - 1) % count,
        };
        usize::from(self.layouts[next])
    }

    /// Writes the [`KEYBOARD_LAYOUT_SET`] value into `out`.
    pub fn write<'a>(&self, out: &'a mut [u8; KEYBOARD_LAYOUT_SET_CAPACITY]) -> &'a str {
        let mut length = 0;
        for (index, number) in self.numbers().enumerate() {
            if index > 0 {
                out[length] = b',';
                length += 1;
            }
            let name = KEYBOARD_LAYOUTS[number].name.as_bytes();
            out[length..length + name.len()].copy_from_slice(name);
            length += name.len();
        }
        core::str::from_utf8(&out[..length]).unwrap_or("us")
    }
}

/// The set of layouts `file` holds, the US layout alone when it holds none or one refused.
pub fn keyboard_layout_set(file: &[u8]) -> KeyboardLayoutSet {
    value(file, KEYBOARD_LAYOUT_SET)
        .and_then(|value| KeyboardLayoutSet::parse(value).ok())
        .unwrap_or(KeyboardLayoutSet::DEFAULT)
}

/// The key that switches layouts ([`KEYBOARD_SWITCH_KEY`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SwitchKey {
    /// The key of the profile's OS: Ctrl+Space in the macOS profile, Super+Space otherwise.
    #[default]
    Profile,
    /// Super+Space; with Shift, backwards.
    SuperSpace,
    /// Ctrl+Space; with Shift, backwards.
    CtrlSpace,
    /// Alt then Shift, released with no other key between.
    AltShift,
    /// Ctrl then Shift, released with no other key between.
    CtrlShift,
    /// No key switches.
    None,
}

impl SwitchKey {
    /// Every switch key, in the Keyboard pane's order; its index is its number.
    pub const ALL: [Self; 6] = [
        Self::Profile,
        Self::SuperSpace,
        Self::CtrlSpace,
        Self::AltShift,
        Self::CtrlShift,
        Self::None,
    ];

    /// The values of [`KEYBOARD_SWITCH_KEY`], in [`Self::ALL`]'s order.
    pub const NAMES: [&'static str; 6] = [
        "profile",
        "super+space",
        "ctrl+space",
        "alt+shift",
        "ctrl+shift",
        "none",
    ];

    /// What the Keyboard pane's dropdown shows, in [`Self::ALL`]'s order.
    pub const LABELS: [&'static str; 6] = [
        "Profile's key",
        "Super+Space",
        "Ctrl+Space",
        "Alt+Shift",
        "Ctrl+Shift",
        "None",
    ];

    /// Its number: its index in [`Self::ALL`].
    pub const fn number(self) -> usize {
        self as usize
    }

    /// The switch key numbered `number`, [`Self::Profile`] for a number that is none.
    pub const fn from_number(number: usize) -> Self {
        match number {
            1 => Self::SuperSpace,
            2 => Self::CtrlSpace,
            3 => Self::AltShift,
            4 => Self::CtrlShift,
            5 => Self::None,
            _ => Self::Profile,
        }
    }

    /// Its [`KEYBOARD_SWITCH_KEY`] value.
    pub const fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// The switch key a [`KEYBOARD_SWITCH_KEY`] value names.
    pub fn named(name: &[u8]) -> Option<Self> {
        Self::NAMES
            .iter()
            .position(|known| known.as_bytes() == name)
            .map(Self::from_number)
    }

    /// The key it is in the profile numbered `profile` (an index of [`PROFILES`]):
    /// [`Self::Profile`] resolved.
    pub const fn resolved(self, profile: usize) -> Self {
        match (self, profile) {
            (Self::Profile, 1) => Self::CtrlSpace,
            (Self::Profile, _) => Self::SuperSpace,
            (key, _) => key,
        }
    }

    /// The chord's name as the bar's tooltip says it, `None` for [`Self::None`].
    pub const fn chord(self, profile: usize) -> Option<&'static str> {
        match self.resolved(profile) {
            Self::None => None,
            key => Some(Self::LABELS[key as usize]),
        }
    }
}

/// The switch key `file` holds, [`SwitchKey::Profile`] when none or an unknown one.
pub fn keyboard_switch_key(file: &[u8]) -> SwitchKey {
    value(file, KEYBOARD_SWITCH_KEY)
        .and_then(SwitchKey::named)
        .unwrap_or_default()
}

/// Why a setting could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The key is empty or holds something other than `a`-`z`, `0`-`9`, `.` and `_`.
    InvalidKey,
    /// The value holds something other than printable ASCII.
    InvalidValue,
    /// The file would not fit the buffer or [`LARGEST`].
    TooLarge,
}

/// Whether `key` may name a setting.
pub fn is_key(key: &[u8]) -> bool {
    !key.is_empty()
        && key
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_'))
}

/// Whether `value` may be a setting's value.
pub fn is_value(value: &[u8]) -> bool {
    value.iter().all(|byte| matches!(byte, b' '..=b'~'))
}

fn without_carriage_return(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn body(file: &[u8]) -> &[u8] {
    if file.len() > LARGEST {
        return &[];
    }
    let (first, rest) = match file.iter().position(|&byte| byte == b'\n') {
        Some(end) => (&file[..end], &file[end + 1..]),
        None => (file, &[][..]),
    };
    match without_carriage_return(first) == HEADER {
        true => rest,
        false => &[],
    }
}

/// The settings of a file, in the order of their lines.
#[derive(Debug, Clone)]
pub struct Entries<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Entries<'a> {
    type Item = (&'a [u8], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        while !self.rest.is_empty() {
            let (line, rest) = match self.rest.iter().position(|&byte| byte == b'\n') {
                Some(end) => (&self.rest[..end], &self.rest[end + 1..]),
                None => (self.rest, &[][..]),
            };
            self.rest = rest;
            let line = without_carriage_return(line);
            let Some(equals) = line.iter().position(|&byte| byte == b'=') else {
                continue;
            };
            let (key, value) = (&line[..equals], &line[equals + 1..]);
            if is_key(key) && is_value(value) {
                return Some((key, value));
            }
        }
        None
    }
}

/// The settings `file` holds; none when it is not a settings file of this version.
pub fn entries(file: &[u8]) -> Entries<'_> {
    Entries { rest: body(file) }
}

/// The value of setting `key` in `file`, from its last line.
pub fn value<'a>(file: &'a [u8], key: &str) -> Option<&'a [u8]> {
    entries(file)
        .filter(|(name, _)| *name == key.as_bytes())
        .last()
        .map(|(_, value)| value)
}

/// The index in [`PROFILES`] of the profile `file` holds, if it holds a known one.
pub fn profile(file: &[u8]) -> Option<usize> {
    let chosen = value(file, SHELL_PROFILE)?;
    PROFILES
        .iter()
        .position(|profile| profile.as_bytes() == chosen)
}

struct Writer<'a> {
    out: &'a mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn put(&mut self, parts: &[&[u8]]) -> Result<(), Refused> {
        let needed: usize = parts.iter().map(|part| part.len()).sum();
        if self.length + needed > self.out.len().min(LARGEST) {
            return Err(Refused::TooLarge);
        }
        for part in parts {
            self.out[self.length..self.length + part.len()].copy_from_slice(part);
            self.length += part.len();
        }
        Ok(())
    }
}

/// Writes into `out` the file `file` becomes with setting `key` set to `value`: the header,
/// every other setting `file` holds in its order, and `key` on the line of its first
/// appearance or last; returns the new file's length.
pub fn with_value(file: &[u8], key: &str, value: &[u8], out: &mut [u8]) -> Result<usize, Refused> {
    if !is_key(key.as_bytes()) {
        return Err(Refused::InvalidKey);
    }
    if !is_value(value) {
        return Err(Refused::InvalidValue);
    }
    let mut writer = Writer { out, length: 0 };
    writer.put(&[HEADER, b"\n"])?;
    let mut written = false;
    for (name, old) in entries(file) {
        if name != key.as_bytes() {
            writer.put(&[name, b"=", old, b"\n"])?;
        } else if !written {
            writer.put(&[name, b"=", value, b"\n"])?;
            written = true;
        }
    }
    if !written {
        writer.put(&[key.as_bytes(), b"=", value, b"\n"])?;
    }
    Ok(writer.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_preferences_out_of_range_or_absent_are_their_defaults() {
        assert_eq!(input_preferences(b""), InputPreferences::DEFAULT);
        let file =
            b"eggos-settings 1\nmouse.buttons=left\nmouse.scrolling=natural\nmouse.speed=9\n\
                     keyboard.repeat_delay_ms=100\nkeyboard.repeat_rate=30\n";
        assert_eq!(
            input_preferences(file),
            InputPreferences {
                left_handed: true,
                natural_scrolling: true,
                pointer_speed: 9,
                repeat_delay_ms: 400,
                repeat_rate: 30,
            }
        );
        assert_eq!(
            input_preferences(b"eggos-settings 1\nmouse.speed=99999999\nmouse.buttons=LEFT\n"),
            InputPreferences::DEFAULT
        );
    }

    #[test]
    fn display_accessibility_is_off_unless_the_file_says_true() {
        assert_eq!(display_accessibility(b""), DisplayAccessibility::default());
        let file = b"eggos-settings 1
accessibility.high_contrast=true
                     accessibility.reduce_motion=on
";
        assert_eq!(
            display_accessibility(file),
            DisplayAccessibility {
                high_contrast: true,
                reduce_motion: false,
            }
        );
    }

    fn set(file: &[u8], key: &str, value: &[u8]) -> Vec<u8> {
        let mut out = [0u8; LARGEST];
        let length = with_value(file, key, value, &mut out).unwrap();
        out[..length].to_vec()
    }

    #[test]
    fn a_value_is_read_from_its_last_line() {
        let file =
            b"eggos-settings 1\nshell.profile=windows\ndisplay.scale=2\nshell.profile=linux\n";
        assert_eq!(value(file, SHELL_PROFILE), Some(&b"linux"[..]));
        assert_eq!(value(file, DISPLAY_SCALE), Some(&b"2"[..]));
        assert_eq!(value(file, "missing"), None);
        assert_eq!(profile(file), Some(2));
    }

    #[test]
    fn a_file_of_another_format_or_version_or_too_long_holds_nothing() {
        for file in [
            &b""[..],
            b"shell.profile=windows\n",
            b"eggos-settings 2\nshell.profile=windows\n",
            b"eggos-settings 1 \nshell.profile=windows\n",
            b"\x00\xff\xfe garbage \x7f",
        ] {
            assert_eq!(entries(file).count(), 0, "{file:?}");
            assert_eq!(profile(file), None);
        }
        let mut long = b"eggos-settings 1\nshell.profile=windows\n".to_vec();
        assert_eq!(profile(&long), Some(0));
        long.resize(LARGEST + 1, b'\n');
        assert_eq!(profile(&long), None);
    }

    #[test]
    fn lines_that_are_no_setting_are_skipped_and_carriage_returns_ignored() {
        let file = b"eggos-settings 1\r\n\nno equals\n=empty key\nBad.Key=x\nkey=\x01\nshell.profile=macos\r\nempty=\n";
        let found: Vec<_> = entries(file).collect();
        assert_eq!(
            found,
            [(&b"shell.profile"[..], &b"macos"[..]), (b"empty", b"")]
        );
        assert_eq!(profile(file), Some(1));
        assert_eq!(profile(b"eggos-settings 1\nshell.profile=amiga\n"), None);
    }

    #[test]
    fn setting_a_key_keeps_the_others_in_order_and_replaces_its_first_line() {
        let file = b"eggos-settings 1\nfuture.key=kept\nshell.profile=macos\ndisplay.scale=1\nshell.profile=linux\n";
        assert_eq!(
            set(file, SHELL_PROFILE, b"windows"),
            b"eggos-settings 1\nfuture.key=kept\nshell.profile=windows\ndisplay.scale=1\n"
        );
        assert_eq!(
            set(b"", SHELL_PROFILE, b"linux"),
            b"eggos-settings 1\nshell.profile=linux\n"
        );
        assert_eq!(
            set(b"garbage", DISPLAY_SCALE, b"2"),
            b"eggos-settings 1\ndisplay.scale=2\n"
        );
    }

    #[test]
    fn an_invalid_key_or_value_or_a_file_too_large_is_refused() {
        let mut out = [0u8; 64];
        assert_eq!(
            with_value(b"", "Shell", b"x", &mut out),
            Err(Refused::InvalidKey)
        );
        assert_eq!(
            with_value(b"", "", b"x", &mut out),
            Err(Refused::InvalidKey)
        );
        assert_eq!(
            with_value(b"", SHELL_PROFILE, b"a\nb", &mut out),
            Err(Refused::InvalidValue)
        );
        assert_eq!(
            with_value(b"", SHELL_PROFILE, &[b'x'; 64], &mut out),
            Err(Refused::TooLarge)
        );
        let mut tiny = [0u8; 4];
        assert_eq!(
            with_value(b"", SHELL_PROFILE, b"x", &mut tiny),
            Err(Refused::TooLarge)
        );
    }

    #[test]
    fn each_process_writes_its_own_temporary_file() {
        let mut out = [0u8; TEMPORARY_PATH_CAPACITY];
        assert_eq!(
            temporary_path(42, &mut out),
            "/home/root/.config/eggos/settings.tmp.42"
        );
        assert_eq!(
            temporary_path(0, &mut out),
            "/home/root/.config/eggos/settings.tmp.0"
        );
        assert_eq!(
            temporary_path(u32::MAX, &mut out),
            "/home/root/.config/eggos/settings.tmp.4294967295"
        );
    }

    #[test]
    fn what_is_written_reads_back() {
        let file = set(b"", SHELL_PROFILE, b"windows");
        let file = set(&file, DISPLAY_SCALE, b"2");
        assert_eq!(profile(&file), Some(0));
        assert_eq!(value(&file, DISPLAY_SCALE), Some(&b"2"[..]));
    }

    #[test]
    fn every_byte_string_reads_without_panicking() {
        let mut state = 0x1234_5678_u32;
        for length in 0..300 {
            let mut file = b"eggos-settings 1\n".to_vec();
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                file.push(b"=\n\rab.z_09 \xff"[(state % 12) as usize]);
            }
            let _ = entries(&file).count();
            let _ = profile(&file);
            let mut out = [0u8; LARGEST];
            let _ = with_value(&file, SHELL_PROFILE, b"linux", &mut out);
        }
    }

    #[test]
    fn stale_temporary_files_are_told_by_their_writer_s_id() {
        let mut out = [0u8; TEMPORARY_PATH_CAPACITY];
        let path = temporary_path(4021, &mut out);
        let name = path.rsplit('/').next().unwrap();
        assert_eq!(temporary_owner(name.as_bytes()), Some(4021));
        assert_eq!(temporary_owner(b"settings"), None);
        assert_eq!(temporary_owner(b"settings.tmp."), None);
        assert_eq!(temporary_owner(b"settings.tmp.12x"), None);
        assert_eq!(temporary_owner(b"settings.tmp.99999999999"), None);
        let ps = b"1 0 1 1 R init\n7 1 7 7 R eggsh\n4021 7 7 7 R settings\n";
        assert!(is_listed(ps, 4021));
        assert!(is_listed(ps, 1));
        assert!(!is_listed(ps, 402));
        assert!(!is_listed(ps, 70));
        assert!(!is_listed(b"", 1));
    }

    #[test]
    fn the_keyboard_layout_is_one_of_the_named_layouts() {
        let file = b"eggos-settings 1\nkeyboard.layout=de\n";
        assert_eq!(keyboard_layout(file), Some(11));
        assert_eq!(KEYBOARD_LAYOUTS[11].title, "German");
        assert_eq!(
            keyboard_layout(b"eggos-settings 1\nkeyboard.layout=DE\n"),
            None
        );
        assert_eq!(keyboard_layout(b"eggos-settings 1\n"), None);
        assert_eq!(keyboard_layout_named(b"us"), Some(0));
        for (index, layout) in KEYBOARD_LAYOUTS.iter().enumerate() {
            assert_eq!(keyboard_layout_named(layout.name.as_bytes()), Some(index));
            assert!(is_key(KEYBOARD_LAYOUT.as_bytes()) && is_value(layout.name.as_bytes()));
        }
    }

    #[test]
    fn the_layouts_table_has_twenty_rows_with_distinct_two_letter_codes() {
        assert_eq!(KEYBOARD_LAYOUTS.len(), 20);
        assert_eq!(keyboard_layout_named(b"us-mac"), Some(2));
        assert_eq!(keyboard_layout_named(b"gb"), Some(3));
        let ids: [&str; 20] = [
            "us", "us-intl", "us-mac", "gb", "us-dvorak", "us-colemak", "dk", "fi", "fr", "be",
            "ca", "de", "ch", "it", "no", "br", "pt", "es", "latam", "se",
        ];
        for (index, layout) in KEYBOARD_LAYOUTS.iter().enumerate() {
            assert_eq!(layout.name, ids[index]);
            assert_eq!(layout.code.len(), 2, "{}", layout.name);
            assert!(layout.code.bytes().all(|byte| byte.is_ascii_uppercase()));
            for other in &KEYBOARD_LAYOUTS[index + 1..] {
                assert_ne!(layout.code, other.code);
            }
        }
        assert_eq!(KEYBOARD_LAYOUTS[10].dead_keys, "` ^ ¨ ~ ¸");
    }

    #[test]
    fn a_layout_set_is_one_to_four_known_layouts_none_twice() {
        let file = b"eggos-settings 1\nkeyboard.layouts=us,de\nkeyboard.layout=de\n";
        let set = keyboard_layout_set(file);
        assert_eq!(set.numbers().collect::<Vec<_>>(), [0, 11]);
        assert_eq!(keyboard_layout(file), set.get(1));
        assert_eq!(
            KeyboardLayoutSet::parse(b"us,de,fr,es,it"),
            Err(LayoutSetRefusal::TooMany)
        );
        assert_eq!(
            KeyboardLayoutSet::parse(b"us,us"),
            Err(LayoutSetRefusal::Repeated(0))
        );
        assert_eq!(
            KeyboardLayoutSet::parse(b"us,xx"),
            Err(LayoutSetRefusal::Unknown { start: 3, end: 5 })
        );
        assert_eq!(keyboard_layout_set(b"eggos-settings 1\n"), KeyboardLayoutSet::DEFAULT);
        let mut text = [0u8; KEYBOARD_LAYOUT_SET_CAPACITY];
        assert_eq!(set.write(&mut text), "us,de");
        let longest = KeyboardLayoutSet::of(&[4, 5, 1, 2]).unwrap();
        assert_eq!(longest.write(&mut text), "us-dvorak,us-colemak,us-intl,us-mac");
    }

    #[test]
    fn choosing_a_layout_appends_it_or_takes_the_place_of_the_one_in_force() {
        let named = |name: &[u8]| keyboard_layout_named(name).unwrap();
        let mut text = [0u8; KEYBOARD_LAYOUT_SET_CAPACITY];
        let us_de = KeyboardLayoutSet::parse(b"us,de").unwrap();
        assert_eq!(
            us_de.with_chosen(named(b"fr"), named(b"us")).write(&mut text),
            "us,de,fr"
        );
        let full = KeyboardLayoutSet::parse(b"us,de,fr,es").unwrap();
        assert_eq!(full.with_chosen(named(b"fr"), named(b"de")), full);
        assert_eq!(
            full.with_chosen(named(b"it"), named(b"de")).write(&mut text),
            "us,it,fr,es"
        );
        let one = KeyboardLayoutSet::DEFAULT;
        assert_eq!(one.with_chosen(named(b"de"), 0).write(&mut text), "us,de");
        let gb_se = KeyboardLayoutSet::parse(b"gb,se").unwrap();
        assert_eq!(gb_se.keeping(named(b"de")), named(b"gb"));
        assert_eq!(gb_se.keeping(named(b"se")), named(b"se"));
        let three = KeyboardLayoutSet::parse(b"us,de,fr").unwrap();
        assert_eq!(three.next_after(0, false), named(b"de"));
        assert_eq!(three.next_after(named(b"de"), false), named(b"fr"));
        assert_eq!(three.next_after(named(b"fr"), false), 0);
        assert_eq!(three.next_after(0, true), named(b"fr"));
    }

    #[test]
    fn the_switch_key_is_the_profile_s_unless_chosen() {
        assert_eq!(keyboard_switch_key(b"eggos-settings 1\n"), SwitchKey::Profile);
        assert_eq!(
            keyboard_switch_key(b"eggos-settings 1\nkeyboard.switch_key=alt+shift\n"),
            SwitchKey::AltShift
        );
        assert_eq!(SwitchKey::named(b"x"), None);
        assert_eq!(SwitchKey::Profile.resolved(0), SwitchKey::SuperSpace);
        assert_eq!(SwitchKey::Profile.resolved(1), SwitchKey::CtrlSpace);
        assert_eq!(SwitchKey::Profile.resolved(2), SwitchKey::SuperSpace);
        assert_eq!(SwitchKey::Profile.chord(2), Some("Super+Space"));
        assert_eq!(SwitchKey::None.chord(2), None);
        for (index, key) in SwitchKey::ALL.iter().enumerate() {
            assert_eq!(key.number(), index);
            assert_eq!(SwitchKey::named(key.name().as_bytes()), Some(*key));
        }
    }
}
