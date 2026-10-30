//! The US QWERTY keymap of the terminal design (docs/ux/terminal-v0.3.md, "Keyboard"): which
//! keys are held, the modifiers they make, and the bytes a key press sends to a terminal,
//! xterm's encoding. Key repeat ([`KeyRepeat`]) is timed by the caller's clock.

use crate::key;

/// The modifiers in effect for a key press.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers(u8);

impl Modifiers {
    /// None held, Caps Lock off.
    pub const NONE: Self = Self(0);
    /// Either Shift.
    pub const SHIFT: Self = Self(1);
    /// Either Alt.
    pub const ALT: Self = Self(2);
    /// Either Ctrl.
    pub const CTRL: Self = Self(4);
    /// Either Super (the Windows or Command key).
    pub const SUPER: Self = Self(8);
    /// Caps Lock is on.
    pub const CAPS_LOCK: Self = Self(16);
    /// The third-level key: AltGr, or Option in the macOS profile ([`ThirdLevelKeys`], D185).
    pub const ALT_GRAPH: Self = Self(32);

    /// The modifiers of `bits`, as [`Self::bits`] gives them.
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & 63)
    }

    /// The modifiers as bits: 1 Shift, 2 Alt, 4 Ctrl, 8 Super, 16 Caps Lock, 32 the third
    /// level (AltGr).
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every modifier of `other` is in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// `self` with `other`'s modifiers too.
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn xterm_parameter(self) -> u8 {
        1 + (self.0 & 1) + (self.0 & 2) + (self.0 & 4)
    }
}

/// How the arrow keys are sent: the terminal's cursor key mode (DECCKM).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorKeys {
    /// `ESC [ A` and so on.
    #[default]
    Normal,
    /// `ESC O A` and so on, once a program asked with `CSI ? 1 h`.
    Application,
}

/// The bytes one key press sends, at most [`KeyBytes::CAPACITY`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyBytes {
    bytes: [u8; KeyBytes::CAPACITY],
    length: u8,
}

impl KeyBytes {
    /// No bytes.
    pub const EMPTY: Self = Self::of(&[]);

    /// The longest sequence a key sends (`ESC [ 1 5 ; 8 ~` is 7).
    pub const CAPACITY: usize = 8;

    const fn push(mut self, byte: u8) -> Self {
        if (self.length as usize) < Self::CAPACITY {
            self.bytes[self.length as usize] = byte;
            self.length += 1;
        }
        self
    }

    const fn of(bytes: &[u8]) -> Self {
        let mut sequence = Self {
            bytes: [0; Self::CAPACITY],
            length: 0,
        };
        let mut index = 0;
        while index < bytes.len() {
            sequence = sequence.push(bytes[index]);
            index += 1;
        }
        sequence
    }

    /// Adds `byte` at the end, if there is room for it.
    pub fn push_byte(&mut self, byte: u8) {
        *self = self.push(byte);
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Whether the key sends nothing.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

const ESC: u8 = 0x1b;
const DELETE_CHARACTER: u8 = 0x7f;
const BACKSPACE_CHARACTER: u8 = 0x08;
const CARRIAGE_RETURN: u8 = 0x0d;
const HORIZONTAL_TAB: u8 = 0x09;

const PRINTABLE_CODES: usize = 99;

const fn printable_table() -> [[u8; 2]; PRINTABLE_CODES] {
    let mut table = [[0u8; 2]; PRINTABLE_CODES];
    let rows: [(usize, &[u8], &[u8]); 8] = [
        (2, b"1234567890-=", b"!@#$%^&*()_+"),
        (16, b"qwertyuiop[]", b"QWERTYUIOP{}"),
        (30, b"asdfghjkl;'`", b"ASDFGHJKL:\"~"),
        (43, b"\\zxcvbnm,./", b"|ZXCVBNM<>?"),
        (55, b"*", b"*"),
        (57, b" ", b" "),
        (71, b"789-456+1230.", b"789-456+1230."),
        (98, b"/", b"/"),
    ];
    let mut row = 0;
    while row < rows.len() {
        let (first, unshifted, shifted) = rows[row];
        let mut index = 0;
        while index < unshifted.len() {
            table[first + index] = [unshifted[index], shifted[index]];
            index += 1;
        }
        row += 1;
    }
    table
}

const PRINTABLE: [[u8; 2]; PRINTABLE_CODES] = printable_table();

/// The character key `code` types, unshifted and shifted; `None` for a key that types none.
pub const fn printable(code: u16) -> Option<[u8; 2]> {
    if code as usize >= PRINTABLE_CODES {
        return None;
    }
    let pair = PRINTABLE[code as usize];
    if pair[0] == 0 { None } else { Some(pair) }
}

/// The key and modifiers that type the printable ASCII `character` under the US keymap: the
/// main block's key before the keypad's, Shift only when the character is the key's shifted
/// one; `None` for a character no key types alone.
pub const fn key_for_ascii(character: u8) -> Option<(u16, Modifiers)> {
    let mut code = 0;
    while code < PRINTABLE_CODES {
        let [plain, shifted] = PRINTABLE[code];
        if plain != 0 && plain == character {
            return Some((code as u16, Modifiers::NONE));
        }
        if plain != 0 && shifted == character {
            return Some((code as u16, Modifiers::SHIFT));
        }
        code += 1;
    }
    None
}

/// The control character Ctrl makes of `character`: `@` to `_` and the letters to 0x00-0x1F,
/// a space to 0x00; any other character unchanged.
pub const fn control(character: u8) -> u8 {
    match character {
        b' ' => 0,
        b'@'..=b'_' | b'a'..=b'z' => character & 0x1f,
        other => other,
    }
}

const fn csi(final_byte: u8, number: u8, modifiers: Modifiers) -> KeyBytes {
    let parameter = modifiers.xterm_parameter();
    let mut sequence = KeyBytes::of(&[ESC, b'[']);
    if number >= 10 {
        sequence = sequence.push(b'0' + number / 10);
    }
    if number > 0 {
        sequence = sequence.push(b'0' + number % 10);
    }
    if parameter > 1 {
        if number == 0 {
            sequence = sequence.push(b'1');
        }
        sequence = sequence.push(b';').push(b'0' + parameter);
    }
    sequence.push(final_byte)
}

const fn cursor(final_byte: u8, modifiers: Modifiers, mode: CursorKeys) -> KeyBytes {
    match (modifiers.xterm_parameter(), mode) {
        (1, CursorKeys::Application) => KeyBytes::of(&[ESC, b'O', final_byte]),
        _ => csi(final_byte, 0, modifiers),
    }
}

const fn function_key(final_byte: u8, modifiers: Modifiers) -> KeyBytes {
    match modifiers.xterm_parameter() {
        1 => KeyBytes::of(&[ESC, b'O', final_byte]),
        _ => csi(final_byte, 0, modifiers),
    }
}

const fn view_key(modifiers: Modifiers) -> bool {
    modifiers.xterm_parameter() == 2
}

const fn with_alt(sequence: KeyBytes, modifiers: Modifiers) -> KeyBytes {
    if !modifiers.contains(Modifiers::ALT) || sequence.is_empty() {
        return sequence;
    }
    let mut prefixed = KeyBytes::of(&[ESC]);
    let mut index = 0;
    while index < sequence.length as usize {
        prefixed = prefixed.push(sequence.bytes[index]);
        index += 1;
    }
    prefixed
}

/// The bytes a press of key `code` sends with `modifiers`, as the terminal design's table says:
/// characters (Shift, Caps Lock for letters only, Ctrl to control characters), Enter, Backspace,
/// Tab and Esc (Alt adds an `ESC` before them), and the cursor, editing and function keys in
/// xterm's form, with their modifiers as its parameter. Nothing for modifiers, lock keys, keys
/// pressed with Super, Shift with Page Up, Page Down, Home or End (the terminal's scrollback
/// keys) and keys the map does not know.
pub const fn encode(code: u16, modifiers: Modifiers, mode: CursorKeys) -> KeyBytes {
    if modifiers.contains(Modifiers::SUPER) {
        return KeyBytes::of(&[]);
    }
    let shift = modifiers.contains(Modifiers::SHIFT);
    let ctrl = modifiers.contains(Modifiers::CTRL);
    if let Some([unshifted, shifted]) = printable(code) {
        let mut character = if shift { shifted } else { unshifted };
        if modifiers.contains(Modifiers::CAPS_LOCK) && character.is_ascii_alphabetic() {
            character ^= 0x20;
        }
        if ctrl {
            character = control(character);
        }
        return with_alt(KeyBytes::of(&[character]), modifiers);
    }
    let plain = match code {
        key::ENTER | key::KEYPAD_ENTER => KeyBytes::of(&[CARRIAGE_RETURN]),
        key::BACKSPACE if ctrl => KeyBytes::of(&[BACKSPACE_CHARACTER]),
        key::BACKSPACE => KeyBytes::of(&[DELETE_CHARACTER]),
        key::TAB if shift => KeyBytes::of(&[ESC, b'[', b'Z']),
        key::TAB => KeyBytes::of(&[HORIZONTAL_TAB]),
        key::ESC => KeyBytes::of(&[ESC]),
        _ => KeyBytes::of(&[]),
    };
    if !plain.is_empty() {
        return with_alt(plain, modifiers);
    }
    match code {
        key::UP => cursor(b'A', modifiers, mode),
        key::DOWN => cursor(b'B', modifiers, mode),
        key::RIGHT => cursor(b'C', modifiers, mode),
        key::LEFT => cursor(b'D', modifiers, mode),
        key::HOME | key::END | key::PAGE_UP | key::PAGE_DOWN if view_key(modifiers) => {
            KeyBytes::of(&[])
        }
        key::HOME => csi(b'H', 0, modifiers),
        key::END => csi(b'F', 0, modifiers),
        key::INSERT => csi(b'~', 2, modifiers),
        key::DELETE => csi(b'~', 3, modifiers),
        key::PAGE_UP => csi(b'~', 5, modifiers),
        key::PAGE_DOWN => csi(b'~', 6, modifiers),
        key::F1 => function_key(b'P', modifiers),
        key::F2 => function_key(b'Q', modifiers),
        key::F3 => function_key(b'R', modifiers),
        key::F4 => function_key(b'S', modifiers),
        key::F5 => csi(b'~', 15, modifiers),
        key::F6 => csi(b'~', 17, modifiers),
        key::F7 => csi(b'~', 18, modifiers),
        key::F8 => csi(b'~', 19, modifiers),
        key::F9 => csi(b'~', 20, modifiers),
        key::F10 => csi(b'~', 21, modifiers),
        key::F11 => csi(b'~', 23, modifiers),
        key::F12 => csi(b'~', 24, modifiers),
        _ => KeyBytes::of(&[]),
    }
}

/// What a key event changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyChange {
    Pressed,
    Released,
}

const HELD_WORDS: usize = (key::MAX as usize + 1) / 64;

const MODIFIER_KEYS: [(u16, u8); 8] = [
    (key::LEFT_SHIFT, 1),
    (key::RIGHT_SHIFT, 1 << 1),
    (key::LEFT_ALT, 1 << 2),
    (key::RIGHT_ALT, 1 << 3),
    (key::LEFT_CTRL, 1 << 4),
    (key::RIGHT_CTRL, 1 << 5),
    (key::LEFT_SUPER, 1 << 6),
    (key::RIGHT_SUPER, 1 << 7),
];

/// Which Alt keys reach a layout's third level ([`Modifiers::ALT_GRAPH`], D185).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThirdLevelKeys {
    /// None: the layout has no third level, so both Alt keys are Alt.
    #[default]
    None,
    /// Right Alt is AltGr and no longer Alt; Left Alt stays Alt (the Windows and Linux
    /// profiles).
    RightAlt,
    /// Either Alt key is Option: Alt for shortcuts and the third level for typing (the macOS
    /// profile).
    EitherAlt,
}

/// A keyboard's state: the keys held, the modifiers they make and Caps Lock. All zeros when new,
/// so a `static` of it costs nothing in the kernel image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyboard {
    held: [u64; HELD_WORDS],
    modifier_keys: u8,
    caps_lock: bool,
    third_level: ThirdLevelKeys,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Keyboard {
    /// No key held, Caps Lock off.
    pub const fn new() -> Self {
        Self {
            held: [0; HELD_WORDS],
            modifier_keys: 0,
            caps_lock: false,
            third_level: ThirdLevelKeys::None,
        }
    }

    /// Has `keys` reach the third level from now on.
    pub fn set_third_level_keys(&mut self, keys: ThirdLevelKeys) {
        self.third_level = keys;
    }

    /// Whether Caps Lock is on.
    pub const fn caps_lock(&self) -> bool {
        self.caps_lock
    }

    /// Applies a key event: `value` 1 pressed, 0 released. Ignores what would change nothing:
    /// a press of a key already held (a host's autorepeat), a release of a key not held, a
    /// repeat (`value` 2: repeating is the caller's, [`KeyRepeat`]) and codes above
    /// [`key::MAX`]. Caps Lock toggles when pressed.
    pub fn key(&mut self, code: u16, value: i32) -> Option<KeyChange> {
        if code > key::MAX {
            return None;
        }
        let (word, bit) = (usize::from(code / 64), 1u64 << (code % 64));
        let held = self.held[word] & bit != 0;
        let change = match value {
            1 if !held => KeyChange::Pressed,
            0 if held => KeyChange::Released,
            _ => return None,
        };
        self.held[word] ^= bit;
        if let Some(&(_, mask)) = MODIFIER_KEYS.iter().find(|(modifier, _)| *modifier == code) {
            self.modifier_keys ^= mask;
        }
        if code == key::CAPS_LOCK && change == KeyChange::Pressed {
            self.caps_lock = !self.caps_lock;
        }
        Some(change)
    }

    /// Whether key `code` is held.
    pub fn is_held(&self, code: u16) -> bool {
        code <= key::MAX && self.held[usize::from(code / 64)] & (1 << (code % 64)) != 0
    }

    /// Whether any key is held.
    pub fn any_held(&self) -> bool {
        self.held.iter().any(|&word| word != 0)
    }

    /// The modifiers held and Caps Lock.
    pub const fn modifiers(&self) -> Modifiers {
        let keys = self.modifier_keys;
        let mut bits = 0;
        if keys & 0b11 != 0 {
            bits |= Modifiers::SHIFT.0;
        }
        let (left_alt, right_alt) = (keys & 0b100 != 0, keys & 0b1000 != 0);
        match self.third_level {
            ThirdLevelKeys::None if left_alt || right_alt => bits |= Modifiers::ALT.0,
            ThirdLevelKeys::RightAlt => {
                if left_alt {
                    bits |= Modifiers::ALT.0;
                }
                if right_alt {
                    bits |= Modifiers::ALT_GRAPH.0;
                }
            }
            ThirdLevelKeys::EitherAlt if left_alt || right_alt => {
                bits |= Modifiers::ALT.0 | Modifiers::ALT_GRAPH.0;
            }
            _ => {}
        }
        if keys & 0b11_0000 != 0 {
            bits |= Modifiers::CTRL.0;
        }
        if keys & 0b1100_0000 != 0 {
            bits |= Modifiers::SUPER.0;
        }
        if self.caps_lock {
            bits |= Modifiers::CAPS_LOCK.0;
        }
        Modifiers(bits)
    }
}

/// Key repeat: the last key pressed that sends bytes repeats after a delay, then at an interval,
/// until it is released or another key is pressed. Times are in the caller's clock ticks. All
/// zeros when idle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyRepeat {
    code: u16,
    next: u64,
}

impl KeyRepeat {
    /// Nothing repeating.
    pub const fn new() -> Self {
        Self { code: 0, next: 0 }
    }

    /// A key was pressed at `now`: any repeat stops, and `code` repeats from `now + delay` if
    /// `repeats` (it sends bytes).
    pub fn pressed(&mut self, code: u16, repeats: bool, now: u64, delay: u64) {
        self.code = if repeats { code } else { 0 };
        self.next = now.saturating_add(delay);
    }

    /// Key `code` was released: it stops repeating if it was.
    pub fn released(&mut self, code: u16) {
        if self.code == code {
            self.code = 0;
        }
    }

    /// The key due to repeat at `now`, if any; the next repeat is then `interval` after the one
    /// due, or after `now` if polling fell behind (repeats are never sent in a burst).
    pub fn due(&mut self, now: u64, interval: u64) -> Option<u16> {
        if self.code == 0 || now < self.next {
            return None;
        }
        let after_due = self.next.saturating_add(interval);
        self.next = if after_due > now {
            after_due
        } else {
            now.saturating_add(interval)
        };
        Some(self.code)
    }

    /// When the next repeat is due, if a key repeats.
    pub fn next(&self) -> Option<u64> {
        (self.code != 0).then_some(self.next)
    }
}
