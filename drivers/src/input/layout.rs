//! Keyboard layouts (D185, docs/ux/keyboard-v0.3.md): what each key of the main block types at
//! its four levels (plain, Shift, the third level reached with AltGr or Option, the third level
//! with Shift) in each layout of [`LAYOUTS`], dead keys whose accent waits for the next key, and
//! a [`Typist`] that turns key presses into the characters they type. The keypad and the space
//! bar type what the US keymap ([`super::keymap`]) says on every layout, except the keypad's
//! decimal key.

use super::key;
use super::keymap::{self, CursorKeys, KeyBytes, Modifiers};

const GRAVE: u16 = 0x300;
const ACUTE: u16 = 0x301;
const CIRCUMFLEX: u16 = 0x302;
const TILDE: u16 = 0x303;
const DIAERESIS: u16 = 0x308;
const RING: u16 = 0x30a;
const CEDILLA: u16 = 0x327;

const COMBINING_MARKS: core::ops::RangeInclusive<u16> = 0x300..=0x36f;

const ROWS: [(u16, usize); 6] = [(2, 12), (16, 12), (30, 12), (43, 11), (86, 1), (89, 1)];
const SLOTS: usize = 49;
const LEVELS: usize = 4;
const WIDEST_ROW: usize = 12;
const KEYPAD_DOT: u16 = key::KEYPAD_DOT;
const SPACE: u16 = b' ' as u16;

type Levels = [u16; LEVELS];
type Keys = [Levels; SLOTS];

const fn slot(code: u16) -> Option<usize> {
    let mut row = 0;
    let mut first_slot = 0;
    while row < ROWS.len() {
        let (first, count) = ROWS[row];
        if code >= first && ((code - first) as usize) < count {
            return Some(first_slot + (code - first) as usize);
        }
        first_slot += count;
        row += 1;
    }
    None
}

const fn decode_row(text: &str, count: usize) -> [u16; WIDEST_ROW] {
    let bytes = text.as_bytes();
    let mut out = [0u16; WIDEST_ROW];
    let mut at = 0;
    let mut characters = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let (codepoint, length) = if byte < 0x80 {
            (byte as u32, 1)
        } else if byte & 0xe0 == 0xc0 {
            (
                ((byte & 0x1f) as u32) << 6 | (bytes[at + 1] & 0x3f) as u32,
                2,
            )
        } else if byte & 0xf0 == 0xe0 {
            (
                ((byte & 0x0f) as u32) << 12
                    | ((bytes[at + 1] & 0x3f) as u32) << 6
                    | (bytes[at + 2] & 0x3f) as u32,
                3,
            )
        } else {
            panic!("a layout row holds a character beyond the Basic Multilingual Plane")
        };
        assert!(
            characters < count,
            "a layout row has more characters than keys"
        );
        out[characters] = if codepoint == SPACE as u32 {
            0
        } else {
            codepoint as u16
        };
        characters += 1;
        at += length;
    }
    assert!(
        characters == count,
        "a layout row has fewer characters than keys"
    );
    out
}

const fn keys(rows: [[&str; LEVELS]; 6]) -> Keys {
    let mut keys = [[0u16; LEVELS]; SLOTS];
    let mut row = 0;
    let mut first_slot = 0;
    while row < ROWS.len() {
        let count = ROWS[row].1;
        let mut level = 0;
        while level < LEVELS {
            let decoded = decode_row(rows[row][level], count);
            let mut index = 0;
            while index < count {
                keys[first_slot + index][level] = decoded[index];
                index += 1;
            }
            level += 1;
        }
        first_slot += count;
        row += 1;
    }
    keys
}

const fn has_third_level(keys: &Keys) -> bool {
    let mut index = 0;
    while index < SLOTS {
        if keys[index][2] != 0 || keys[index][3] != 0 {
            return true;
        }
        index += 1;
    }
    false
}

/// One keyboard layout.
#[derive(Debug)]
pub struct Layout {
    /// Its name: the value of the `keyboard.layout` setting.
    pub name: &'static str,
    keys: &'static Keys,
    keypad_decimal: u8,
    ascii_accents: bool,
    acute_cedilla: bool,
    third_level: bool,
}

impl Layout {
    const fn new(
        name: &'static str,
        keys: &'static Keys,
        keypad_decimal: u8,
        ascii_accents: bool,
        acute_cedilla: bool,
    ) -> Self {
        Self {
            name,
            keys,
            keypad_decimal,
            ascii_accents,
            acute_cedilla,
            third_level: has_third_level(keys),
        }
    }

    /// Whether some key types something at the third level.
    pub const fn has_third_level(&self) -> bool {
        self.third_level
    }

    /// Whether some key is a dead key for `accent`, given as the spacing accent
    /// ([`DEAD_ACCENTS`]).
    pub fn has_dead_key(&self, accent: char) -> bool {
        let Some(mark) = DEAD_ACCENTS
            .iter()
            .position(|&dead| dead == accent)
            .map(|index| DEAD_MARKS[index])
        else {
            return false;
        };
        self.keys.iter().flatten().any(|&symbol| symbol == mark)
    }

    fn levels(&self, code: u16) -> Option<Levels> {
        if let Some(slot) = slot(code) {
            return Some(self.keys[slot]);
        }
        if code == KEYPAD_DOT {
            let decimal = u16::from(self.keypad_decimal);
            return Some([decimal, decimal, 0, 0]);
        }
        let [plain, shifted] = keymap::printable(code)?;
        Some([u16::from(plain), u16::from(shifted), 0, 0])
    }

    /// Whether key `code` types characters in this layout (the main block, the space bar and
    /// the keypad's character keys).
    pub fn is_character_key(&self, code: u16) -> bool {
        self.levels(code)
            .is_some_and(|levels| levels.iter().any(|&symbol| symbol != 0))
    }

    /// What key `code` types at its first two levels with `modifiers`' Shift and Caps Lock: a
    /// character, a dead key's combining mark, or 0.
    fn base_symbol(&self, code: u16, modifiers: Modifiers) -> u16 {
        self.levels(code)
            .map_or(0, |levels| pick(levels[0], levels[1], modifiers))
    }

    /// What key `code` types at the third level with `modifiers`' Shift and Caps Lock: a
    /// character, a dead key's combining mark, or 0.
    fn third_symbol(&self, code: u16, modifiers: Modifiers) -> u16 {
        self.levels(code).map_or(0, |levels| {
            let symbol = pick(levels[2], levels[3], modifiers);
            if symbol == 0 { levels[2] } else { symbol }
        })
    }

    fn spacing_accent(&self, mark: u16) -> char {
        match mark {
            ACUTE if self.ascii_accents => '\'',
            DIAERESIS if self.ascii_accents => '"',
            _ => DEAD_MARKS
                .iter()
                .position(|&dead| dead == mark)
                .map_or('?', |index| DEAD_ACCENTS[index]),
        }
    }

    fn compose(&self, mark: u16, base: char) -> Option<char> {
        if mark == ACUTE && self.acute_cedilla {
            match base {
                'c' => return Some('ç'),
                'C' => return Some('Ç'),
                _ => {}
            }
        }
        let (bases, accented) = COMPOSITIONS
            .iter()
            .find(|(composed_mark, _, _)| *composed_mark == mark)
            .map(|(_, bases, accented)| (*bases, *accented))?;
        bases
            .chars()
            .zip(accented.chars())
            .find(|(candidate, _)| *candidate == base)
            .map(|(_, composed)| composed)
    }
}

fn pick(plain: u16, shifted: u16, modifiers: Modifiers) -> u16 {
    let shift = modifiers.contains(Modifiers::SHIFT);
    let caps = modifiers.contains(Modifiers::CAPS_LOCK) && is_letter_pair(plain, shifted);
    if shift != caps { shifted } else { plain }
}

fn is_letter_pair(plain: u16, shifted: u16) -> bool {
    let (Some(plain), Some(shifted)) = (
        char::from_u32(u32::from(plain)),
        char::from_u32(u32::from(shifted)),
    ) else {
        return false;
    };
    plain.is_lowercase() && shifted.is_uppercase()
}

/// The spacing accents of the dead keys, in the order the Keyboard pane lists them, then the
/// two no layout here has (ring and cedilla).
pub const DEAD_ACCENTS: [char; 7] = ['´', '`', '^', '¨', '~', '°', '¸'];

const DEAD_MARKS: [u16; 7] = [ACUTE, GRAVE, CIRCUMFLEX, DIAERESIS, TILDE, RING, CEDILLA];

const COMPOSITIONS: [(u16, &str, &str); 7] = [
    (GRAVE, "AaEeIiOoUu", "ÀàÈèÌìÒòÙù"),
    (ACUTE, "AaEeIiOoUuYy", "ÁáÉéÍíÓóÚúÝý"),
    (CIRCUMFLEX, "AaEeIiOoUu", "ÂâÊêÎîÔôÛû"),
    (TILDE, "AaNnOo", "ÃãÑñÕõ"),
    (DIAERESIS, "AaEeIiOoUuy", "ÄäËëÏïÖöÜüÿ"),
    (RING, "Aa", "Åå"),
    (CEDILLA, "Cc", "Çç"),
];

static US: Keys = keys([
    [
        "1234567890-=",
        "!@#$%^&*()_+",
        "            ",
        "            ",
    ],
    [
        "qwertyuiop[]",
        "QWERTYUIOP{}",
        "            ",
        "            ",
    ],
    [
        "asdfghjkl;'`",
        "ASDFGHJKL:\"~",
        "            ",
        "            ",
    ],
    ["\\zxcvbnm,./", "|ZXCVBNM<>?", "           ", "           "],
    ["\\", "|", " ", " "],
    [" ", " ", " ", " "],
]);

static US_INTERNATIONAL: Keys = keys([
    [
        "1234567890-=",
        "!@#$%\u{302}&*()_+",
        "¡²³¤€¼½¾  ¥×",
        "¹  £       ÷",
    ],
    [
        "qwertyuiop[]",
        "QWERTYUIOP{}",
        "äåé®þüúíóö«»",
        "ÄÅÉ ÞÜÚÍÓÖ  ",
    ],
    [
        "asdfghjkl;\u{301}\u{300}",
        "ASDFGHJKL:\u{308}\u{303}",
        "áßð     ø¶´ ",
        "Á§Ð     Ø°¨ ",
    ],
    ["\\zxcvbnm,./", "|ZXCVBNM<>?", "¬æ ©  ñµç ¿", "¦Æ ¢  Ñ Ç  "],
    ["\\", "|", "¬", "¦"],
    [" ", " ", " ", " "],
]);

static UNITED_KINGDOM: Keys = keys([
    [
        "1234567890-=",
        "!\"£$%^&*()_+",
        "   €        ",
        "            ",
    ],
    [
        "qwertyuiop[]",
        "QWERTYUIOP{}",
        "  é   úíó   ",
        "  É   ÚÍÓ   ",
    ],
    [
        "asdfghjkl;'`",
        "ASDFGHJKL:@¬",
        "á          ¦",
        "Á           ",
    ],
    ["#zxcvbnm,./", "~ZXCVBNM<>?", "           ", "           "],
    ["\\", "|", " ", " "],
    [" ", " ", " ", " "],
]);

static DVORAK: Keys = keys([
    [
        "1234567890[]",
        "!@#$%^&*(){}",
        "            ",
        "            ",
    ],
    [
        "',.pyfgcrl/=",
        "\"<>PYFGCRL?+",
        "            ",
        "            ",
    ],
    [
        "aoeuidhtns-`",
        "AOEUIDHTNS_~",
        "            ",
        "            ",
    ],
    ["\\;qjkxbmwvz", "|:QJKXBMWVZ", "           ", "           "],
    ["\\", "|", " ", " "],
    [" ", " ", " ", " "],
]);

static COLEMAK: Keys = keys([
    [
        "1234567890-=",
        "!@#$%^&*()_+",
        "            ",
        "            ",
    ],
    [
        "qwfpgjluy;[]",
        "QWFPGJLUY:{}",
        "            ",
        "            ",
    ],
    [
        "arstdhneio'`",
        "ARSTDHNEIO\"~",
        "            ",
        "            ",
    ],
    ["\\zxcvbkm,./", "|ZXCVBKM<>?", "           ", "           "],
    ["\\", "|", " ", " "],
    [" ", " ", " ", " "],
]);

static DANISH: Keys = keys([
    [
        "1234567890+\u{301}",
        "!\"#¤%&/()=?\u{300}",
        " @£$€ {[]} |",
        "            ",
    ],
    [
        "qwertyuiopå\u{308}",
        "QWERTYUIOPÅ\u{302}",
        "  €        \u{303}",
        "            ",
    ],
    [
        "asdfghjklæø½",
        "ASDFGHJKLÆØ§",
        "            ",
        "            ",
    ],
    ["'zxcvbnm,.-", "*ZXCVBNM;:_", "       µ   ", "           "],
    ["<", ">", "\\", " "],
    [" ", " ", " ", " "],
]);

static SWEDISH: Keys = keys([
    [
        "1234567890+\u{301}",
        "!\"#¤%&/()=?\u{300}",
        " @£$€ {[]}\\ ",
        "            ",
    ],
    [
        "qwertyuiopå\u{308}",
        "QWERTYUIOPÅ\u{302}",
        "  €        \u{303}",
        "            ",
    ],
    [
        "asdfghjklöä§",
        "ASDFGHJKLÖÄ½",
        "            ",
        "            ",
    ],
    ["'zxcvbnm,.-", "*ZXCVBNM;:_", "       µ   ", "           "],
    ["<", ">", "|", " "],
    [" ", " ", " ", " "],
]);

static FRENCH: Keys = keys([
    [
        "&é\"'(-è_çà)=",
        "1234567890°+",
        " ~#{[|`\\^@]}",
        "            ",
    ],
    [
        "azertyuiop\u{302}$",
        "AZERTYUIOP\u{308}£",
        "  €        ¤",
        "            ",
    ],
    [
        "qsdfghjklmù²",
        "QSDFGHJKLM% ",
        "            ",
        "            ",
    ],
    ["*wxcvbn,;:!", "µWXCVBN?./§", "           ", "           "],
    ["<", ">", " ", " "],
    [" ", " ", " ", " "],
]);

static GERMAN: Keys = keys([
    [
        "1234567890ß\u{301}",
        "!\"§$%&/()=?\u{300}",
        " ²³   {[]}\\ ",
        "            ",
    ],
    [
        "qwertzuiopü+",
        "QWERTZUIOPÜ*",
        "@ €        ~",
        "            ",
    ],
    [
        "asdfghjklöä\u{302}",
        "ASDFGHJKLÖÄ°",
        "            ",
        "            ",
    ],
    ["#yxcvbnm,.-", "'YXCVBNM;:_", "       µ   ", "           "],
    ["<", ">", "|", " "],
    [" ", " ", " ", " "],
]);

static ITALIAN: Keys = keys([
    [
        "1234567890'ì",
        "!\"£$%&/()=?^",
        "          `~",
        "            ",
    ],
    [
        "qwertyuiopè+",
        "QWERTYUIOPé*",
        "  €       []",
        "          {}",
    ],
    [
        "asdfghjklòà\\",
        "ASDFGHJKLç°|",
        "         @# ",
        "            ",
    ],
    ["ùzxcvbnm,.-", "§ZXCVBNM;:_", "           ", "           "],
    ["<", ">", " ", " "],
    [" ", " ", " ", " "],
]);

static NORWEGIAN: Keys = keys([
    [
        "1234567890+\\",
        "!\"#¤%&/()=?\u{300}",
        " @£$€ {[]} \u{301}",
        "            ",
    ],
    [
        "qwertyuiopå\u{308}",
        "QWERTYUIOPÅ\u{302}",
        "  €        \u{303}",
        "            ",
    ],
    [
        "asdfghjkløæ|",
        "ASDFGHJKLØÆ§",
        "            ",
        "            ",
    ],
    ["'zxcvbnm,.-", "*ZXCVBNM;:_", "       µ   ", "           "],
    ["<", ">", " ", " "],
    [" ", " ", " ", " "],
]);

static BRAZILIAN: Keys = keys([
    [
        "1234567890-=",
        "!@#$%\u{308}&*()_+",
        "¹²³£¢¬     §",
        "            ",
    ],
    [
        "qwertyuiop\u{301}[",
        "QWERTYUIOP\u{300}{",
        "/?€        ª",
        "            ",
    ],
    [
        "asdfghjklç\u{303}'",
        "ASDFGHJKLÇ\u{302}\"",
        "            ",
        "            ",
    ],
    ["]zxcvbnm,.;", "}ZXCVBNM<>:", "º          ", "           "],
    ["\\", "|", " ", " "],
    ["/", "?", "°", " "],
]);

static SPANISH: Keys = keys([
    [
        "1234567890'¡",
        "!\"·$%&/()=?¿",
        "|@#~€¬      ",
        "            ",
    ],
    [
        "qwertyuiop\u{300}+",
        "QWERTYUIOP\u{302}*",
        "  €       []",
        "            ",
    ],
    [
        "asdfghjklñ\u{301}º",
        "ASDFGHJKLÑ\u{308}ª",
        "          {\\",
        "            ",
    ],
    ["çzxcvbnm,.-", "ÇZXCVBNM;:_", "}          ", "           "],
    ["<", ">", " ", " "],
    [" ", " ", " ", " "],
]);

/// Every layout, in the order of the settings file's `KEYBOARD_LAYOUTS` (the Keyboard pane's
/// list): a layout's number is its index here. `fi` and `se` share one table.
pub static LAYOUTS: [Layout; 14] = [
    Layout::new("us", &US, b'.', false, false),
    Layout::new("us-intl", &US_INTERNATIONAL, b'.', true, true),
    Layout::new("gb", &UNITED_KINGDOM, b'.', false, false),
    Layout::new("us-dvorak", &DVORAK, b'.', false, false),
    Layout::new("us-colemak", &COLEMAK, b'.', false, false),
    Layout::new("dk", &DANISH, b',', false, false),
    Layout::new("fi", &SWEDISH, b',', false, false),
    Layout::new("fr", &FRENCH, b',', false, false),
    Layout::new("de", &GERMAN, b',', false, false),
    Layout::new("it", &ITALIAN, b',', false, false),
    Layout::new("no", &NORWEGIAN, b',', false, false),
    Layout::new("br", &BRAZILIAN, b',', false, true),
    Layout::new("es", &SPANISH, b',', false, false),
    Layout::new("se", &SWEDISH, b',', false, false),
];

/// The number of the US layout, the default.
pub const US_LAYOUT: usize = 0;

/// The number of the layout named `name`.
pub fn named(name: &[u8]) -> Option<usize> {
    LAYOUTS
        .iter()
        .position(|layout| layout.name.as_bytes() == name)
}

/// The layout numbered `number`, the US layout for a number that is none.
pub fn layout(number: usize) -> &'static Layout {
    LAYOUTS.get(number).unwrap_or(&LAYOUTS[US_LAYOUT])
}

/// The characters one key press types: none, one, or two (a dead key's accent then a
/// character it does not combine with).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Typed {
    characters: [char; 2],
    count: u8,
}

impl Typed {
    /// Nothing typed.
    pub const NOTHING: Self = Self {
        characters: ['\0'; 2],
        count: 0,
    };

    const fn one(character: char) -> Self {
        Self {
            characters: [character, '\0'],
            count: 1,
        }
    }

    const fn two(first: char, second: char) -> Self {
        Self {
            characters: [first, second],
            count: 2,
        }
    }

    /// The characters, in the order typed.
    pub fn characters(&self) -> &[char] {
        &self.characters[..usize::from(self.count)]
    }

    /// The last character, which goes with the key's own event.
    pub fn last(&self) -> Option<char> {
        self.characters().last().copied()
    }

    /// The character typed before the last, which goes with an event of no key of its own.
    pub fn before_last(&self) -> Option<char> {
        (self.count == 2).then_some(self.characters[0])
    }
}

/// What a key press did ([`Typist::press`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// It typed characters; `meta` when Alt was held for a key that typed from its first two
    /// levels, so the terminal sends `ESC` before them.
    Typed { typed: Typed, meta: bool },
    /// A dead key: its accent waits for the next key, and nothing is typed.
    Dead,
    /// Backspace took back a waiting accent: the key does nothing more.
    Cancelled,
    /// A character key held with Ctrl or Super: the character its first two levels type, for a
    /// control character or a shortcut (`'\0'` for a dead key).
    Shortcut(char),
    /// Any other key: a modifier, a lock, or a key the layout has no character for.
    Other,
}

const ENTER_CHARACTER: u32 = 0x0d;
const TAB_CHARACTER: u32 = 0x09;
const BACKSPACE_CHARACTER: u32 = 0x08;
const ESCAPE_CHARACTER: u32 = 0x1b;
const ESC: u8 = 0x1b;

impl Press {
    /// The character a desktop window's `Key` record carries for the press of key `code`
    /// with `modifiers`: the last one typed, a letter's control character with Ctrl, Enter,
    /// Tab, Backspace and Esc their control characters, else 0.
    pub fn character(&self, code: u16, modifiers: Modifiers) -> u32 {
        match *self {
            Press::Typed { typed, .. } => typed.last().map_or(0, u32::from),
            Press::Dead | Press::Cancelled => 0,
            Press::Shortcut(character)
                if modifiers.contains(Modifiers::CTRL) && character.is_ascii_alphabetic() =>
            {
                character as u32 & 0x1f
            }
            Press::Shortcut(character) => u32::from(character),
            Press::Other => match code {
                key::ENTER | key::KEYPAD_ENTER => ENTER_CHARACTER,
                key::TAB => TAB_CHARACTER,
                key::BACKSPACE => BACKSPACE_CHARACTER,
                key::ESC => ESCAPE_CHARACTER,
                _ => 0,
            },
        }
    }

    /// The bytes the press of key `code` with `modifiers` sends to a terminal in cursor key
    /// `mode`: the typed characters in UTF-8 (after `ESC` for `meta`), a control character for
    /// Ctrl with an ASCII character, nothing for a dead key, and the US keymap's sequences
    /// ([`keymap::encode`]) for every other key.
    pub fn terminal_bytes(&self, code: u16, modifiers: Modifiers, mode: CursorKeys) -> KeyBytes {
        let mut bytes = KeyBytes::default();
        match *self {
            Press::Typed { typed, meta } => {
                if meta && typed.count > 0 {
                    bytes.push_byte(ESC);
                }
                for &character in typed.characters() {
                    let mut encoded = [0u8; 4];
                    for &byte in character.encode_utf8(&mut encoded).as_bytes() {
                        bytes.push_byte(byte);
                    }
                }
            }
            Press::Dead | Press::Cancelled => {}
            Press::Shortcut(character)
                if modifiers.contains(Modifiers::CTRL)
                    && !modifiers.contains(Modifiers::SUPER)
                    && character.is_ascii() =>
            {
                if modifiers.contains(Modifiers::ALT) {
                    bytes.push_byte(ESC);
                }
                bytes.push_byte(keymap::control(character as u8));
            }
            Press::Shortcut(_) => {}
            Press::Other => bytes = keymap::encode(code, modifiers, mode),
        }
        bytes
    }
}

/// A keyboard's typing state: its layout and the dead key's accent waiting for the next key.
/// All zeros (the US layout, nothing waiting) when new.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Typist {
    layout: u8,
    pending: u16,
}

impl Typist {
    /// The US layout, nothing waiting.
    pub const fn new() -> Self {
        Self {
            layout: 0,
            pending: 0,
        }
    }

    /// The layout's number.
    pub const fn layout_number(&self) -> usize {
        self.layout as usize
    }

    /// The layout.
    pub fn layout(&self) -> &'static Layout {
        layout(self.layout_number())
    }

    /// Types with layout `number` from now on (the US layout for a number that is none); a
    /// waiting accent is dropped.
    pub fn set_layout(&mut self, number: usize) {
        self.layout = if number < LAYOUTS.len() {
            number as u8
        } else {
            US_LAYOUT as u8
        };
        self.pending = 0;
    }

    /// Whether a dead key's accent waits for the next key.
    pub const fn is_pending(&self) -> bool {
        self.pending != 0
    }

    /// What pressing key `code` with `modifiers` types, the waiting accent taken into account:
    /// a dead key's accent waits; the next character key types the accented character, or the
    /// accent and then its own character, or the accent alone after Space or the same dead key
    /// again; Backspace takes the accent back; a modifier or lock key leaves it waiting; any
    /// other key drops it.
    pub fn press(&mut self, code: u16, modifiers: Modifiers) -> Press {
        if is_modifier_or_lock(code) {
            return Press::Other;
        }
        let layout = self.layout();
        if !layout.is_character_key(code) {
            let cancelled = code == key::BACKSPACE && self.is_pending();
            self.pending = 0;
            return if cancelled {
                Press::Cancelled
            } else {
                Press::Other
            };
        }
        let base = layout.base_symbol(code, modifiers);
        if modifiers.contains(Modifiers::CTRL) || modifiers.contains(Modifiers::SUPER) {
            self.pending = 0;
            return Press::Shortcut(spacing_or_nothing(base));
        }
        let third = match modifiers.contains(Modifiers::ALT_GRAPH) {
            true => layout.third_symbol(code, modifiers),
            false => 0,
        };
        let (symbol, from_third_level) = match third {
            0 if modifiers.contains(Modifiers::ALT_GRAPH)
                && !modifiers.contains(Modifiers::ALT) =>
            {
                self.pending = 0;
                return Press::Typed {
                    typed: Typed::NOTHING,
                    meta: false,
                };
            }
            0 => (base, false),
            third => (third, true),
        };
        if symbol == 0 {
            self.pending = 0;
            return Press::Typed {
                typed: Typed::NOTHING,
                meta: false,
            };
        }
        let meta = modifiers.contains(Modifiers::ALT) && !from_third_level;
        if meta {
            self.pending = 0;
            return Press::Typed {
                typed: char::from_u32(u32::from(symbol))
                    .filter(|_| !COMBINING_MARKS.contains(&symbol))
                    .map_or(Typed::NOTHING, Typed::one),
                meta,
            };
        }
        let typed = self.type_symbol(layout, symbol);
        match typed {
            None => Press::Dead,
            Some(typed) => Press::Typed { typed, meta },
        }
    }

    /// What key `code` types with `modifiers` without the waiting accent, as a key's release
    /// reports it: its character at the level the modifiers reach, 0 for a dead key or none.
    pub fn character_of(&self, code: u16, modifiers: Modifiers) -> u32 {
        let layout = self.layout();
        let third = match modifiers.contains(Modifiers::ALT_GRAPH) {
            true => layout.third_symbol(code, modifiers),
            false => 0,
        };
        let symbol = match third {
            0 => layout.base_symbol(code, modifiers),
            third => third,
        };
        match COMBINING_MARKS.contains(&symbol) {
            true => 0,
            false => u32::from(symbol),
        }
    }

    fn type_symbol(&mut self, layout: &Layout, symbol: u16) -> Option<Typed> {
        let pending = core::mem::take(&mut self.pending);
        if COMBINING_MARKS.contains(&symbol) {
            return match pending {
                0 => {
                    self.pending = symbol;
                    None
                }
                same if same == symbol => Some(Typed::one(layout.spacing_accent(same))),
                other => {
                    self.pending = symbol;
                    Some(Typed::one(layout.spacing_accent(other)))
                }
            };
        }
        let character = char::from_u32(u32::from(symbol)).unwrap_or('?');
        Some(match pending {
            0 => Typed::one(character),
            mark if symbol == SPACE => Typed::one(layout.spacing_accent(mark)),
            mark => match layout.compose(mark, character) {
                Some(composed) => Typed::one(composed),
                None => Typed::two(layout.spacing_accent(mark), character),
            },
        })
    }
}

fn spacing_or_nothing(symbol: u16) -> char {
    match COMBINING_MARKS.contains(&symbol) {
        true => '\0',
        false => char::from_u32(u32::from(symbol)).unwrap_or('\0'),
    }
}

const fn is_modifier_or_lock(code: u16) -> bool {
    matches!(
        code,
        key::LEFT_SHIFT
            | key::RIGHT_SHIFT
            | key::LEFT_CTRL
            | key::RIGHT_CTRL
            | key::LEFT_ALT
            | key::RIGHT_ALT
            | key::LEFT_SUPER
            | key::RIGHT_SUPER
            | key::CAPS_LOCK
            | key::NUM_LOCK
            | key::SCROLL_LOCK
    )
}

#[cfg(test)]
mod tests;
