//! The app's main menu as the window server shows it in the top bar (docs/windows.md
//! `SET_MENU`, `docs/ux/macos-gui-v0.3.md` section 5): the label pool the request carries, the
//! key equivalents' labels spelled out (the bar's font has no `⌘`), and which key press a key
//! equivalent answers.

use crate::keys::{function, modifier};
use eggos_toolkit::input::RECORD_SIZE;
use eggos_toolkit::text::Text;

/// The kind of the event record the server sends when an item of a window's main menu was
/// chosen (`MenuChosen`).
pub const MENU_CHOSEN: u16 = 14;

/// The most bytes a pool holds.
pub const POOL_CAPACITY: usize = 248;
/// The most menus.
pub const MAX_MENUS: usize = 6;
/// The most entries (items and separators) a menu holds.
pub const MAX_ENTRIES: usize = 16;
/// The longest key label.
pub const KEY_LABEL_CAPACITY: usize = 32;

/// A key equivalent's label.
pub type KeyLabel = Text<KEY_LABEL_CAPACITY>;

/// The pool of a `SET_MENU` request, built menu by menu, entry by entry. Nothing that does not
/// fit is added, so the indexes the server reports are those of what was added.
#[derive(Debug, Clone, Copy)]
pub struct Pool {
    bytes: [u8; POOL_CAPACITY],
    length: usize,
    menus: usize,
    entries: usize,
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Pool {
    pub const fn new() -> Self {
        Self {
            bytes: [0; POOL_CAPACITY],
            length: 0,
            menus: 0,
            entries: 0,
        }
    }

    /// The request's pool.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// How many menus it holds.
    pub fn menus(&self) -> usize {
        self.menus
    }

    fn push_cleaned(&mut self, text: &str) {
        for character in text.chars() {
            let shown = match character.is_control() {
                true => ' ',
                false => character,
            };
            let mut encoded = [0u8; 4];
            for &byte in shown.encode_utf8(&mut encoded).as_bytes() {
                self.bytes[self.length] = byte;
                self.length += 1;
            }
        }
    }

    fn append(&mut self, kind: u8, label: &str, key_label: Option<&str>) -> bool {
        let needed = 2 + label.len() + key_label.map_or(0, |key_label| key_label.len() + 1);
        if self.length + needed > POOL_CAPACITY {
            return false;
        }
        self.bytes[self.length] = kind;
        self.length += 1;
        self.push_cleaned(label);
        if let Some(key_label) = key_label {
            self.bytes[self.length] = b'\t';
            self.length += 1;
            self.push_cleaned(key_label);
        }
        self.bytes[self.length] = b'\n';
        self.length += 1;
        true
    }

    /// Starts a menu titled `title`; false when no more fit.
    pub fn menu(&mut self, title: &str) -> bool {
        if self.menus == MAX_MENUS || !self.append(b'M', title, None) {
            return false;
        }
        self.menus += 1;
        self.entries = 0;
        true
    }

    fn entry(&mut self, kind: u8, label: &str, key_label: Option<&str>) -> bool {
        if self.menus == 0 || self.entries == MAX_ENTRIES || !self.append(kind, label, key_label) {
            return false;
        }
        self.entries += 1;
        true
    }

    /// Adds an item to the last menu; false when it does not fit.
    pub fn item(&mut self, label: &str, key_label: &str, enabled: bool) -> bool {
        let kind = if enabled { b'I' } else { b'D' };
        self.entry(kind, label, Some(key_label))
    }

    /// Adds a separator to the last menu; false when it does not fit.
    pub fn separator(&mut self) -> bool {
        self.entry(b'-', "", None)
    }
}

/// A choice of a main menu as a `MenuChosen` record carries it: the server's id of the window
/// whose menu it was, the menu, and the entry in that menu (separators counted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    pub window: u32,
    pub menu: usize,
    pub entry: usize,
}

impl Chosen {
    /// The choice `record` carries; `None` for a record of another kind.
    pub fn from_record(record: &[u8; RECORD_SIZE]) -> Option<Self> {
        if u16::from_ne_bytes([record[0], record[1]]) != MENU_CHOSEN {
            return None;
        }
        let word = |at: usize| {
            u32::from_ne_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]])
        };
        Some(Self {
            window: u32::from(u16::from_ne_bytes([record[2], record[3]])),
            menu: word(8) as usize,
            entry: word(12) as usize,
        })
    }
}

fn key_name(key: u32) -> Option<&'static str> {
    Some(match key {
        0x0d | 0x03 => "Enter",
        0x1b => "Esc",
        0x09 => "Tab",
        0x20 => "Space",
        0x08 | 0x7f | function::DELETE => "Del",
        function::UP => "Up",
        function::DOWN => "Down",
        function::LEFT => "Left",
        function::RIGHT => "Right",
        function::HOME => "Home",
        function::END => "End",
        _ => return None,
    })
}

/// The modifiers a key equivalent `key` with mask `mask` asks for: an upper-case letter asks
/// for Shift too, as AppKit reads it.
pub fn required_modifiers(key: u32, mask: u64) -> u64 {
    let shifted = char::from_u32(key).is_some_and(|key| key.is_ascii_uppercase());
    (mask & modifier::KEY_EQUIVALENT) | if shifted { modifier::SHIFT } else { 0 }
}

/// The label of key equivalent `key` (its first character, 0 for none) with mask `mask`:
/// modifiers in macOS's order joined by `+` (`Ctrl+Alt+Shift+Cmd+K`), a letter upper-cased,
/// named keys spelled out; empty for none.
pub fn key_label(key: u32, mask: u64) -> KeyLabel {
    let mut label = KeyLabel::new();
    if key == 0 {
        return label;
    }
    let required = required_modifiers(key, mask);
    for (flag, name) in [
        (modifier::CONTROL, "Ctrl+"),
        (modifier::OPTION, "Alt+"),
        (modifier::SHIFT, "Shift+"),
        (modifier::COMMAND, "Cmd+"),
    ] {
        if required & flag != 0 {
            label.push_str(name);
        }
    }
    if let Some(name) = key_name(key) {
        label.push_str(name);
    } else if (function::F1..=function::F12).contains(&key) {
        let number = key - function::F1 + 1;
        label.push_str("F");
        if number >= 10 {
            label.push_str("1");
        }
        if let Some(digit) = char::from_digit(number % 10, 10) {
            label.insert(label.len(), digit);
        }
    } else if let Some(character) = char::from_u32(key) {
        label.insert(label.len(), character.to_ascii_uppercase());
    }
    label
}

/// Whether a key press of `typed` (its `charactersIgnoringModifiers`' first character) with
/// `flags` held answers key equivalent `key` with mask `mask`.
pub fn answers(key: u32, mask: u64, typed: u32, flags: u64) -> bool {
    if key == 0 {
        return false;
    }
    let lower = |value: u32| {
        char::from_u32(value).map_or(value, |character| u32::from(character.to_ascii_lowercase()))
    };
    let typed_return = typed == 0x0d && key == 0x03;
    (lower(key) == lower(typed) || typed_return)
        && flags & modifier::KEY_EQUIVALENT == required_modifiers(key, mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_holds_menus_items_and_separators_line_by_line() {
        let mut pool = Pool::new();
        assert!(!pool.item("orphan", "", true));
        assert!(pool.menu("mac-paint"));
        assert!(pool.item("About mac-paint", "", true));
        assert!(pool.separator());
        assert!(pool.item("Quit mac-paint", "Cmd+Q", true));
        assert!(pool.menu("File"));
        assert!(pool.item("Close\twith\ntabs", "Cmd+W", false));
        assert_eq!(
            pool.as_bytes(),
            b"Mmac-paint\nIAbout mac-paint\t\n-\nIQuit mac-paint\tCmd+Q\nMFile\nDClose with tabs\tCmd+W\n"
        );
        assert_eq!(pool.menus(), 2);
    }

    #[test]
    fn every_control_character_becomes_a_space_as_the_server_wants() {
        let mut pool = Pool::new();
        assert!(pool.menu("A\u{85}B"));
        assert!(pool.item("C\u{9f}\u{7f}D", "E\u{80}", true));
        assert!(pool.item("caf\u{e9} \u{a0}", "", true));
        assert_eq!(
            pool.as_bytes(),
            "MA B\nIC  D\tE \nIcaf\u{e9} \u{a0}\t\n".as_bytes()
        );
        assert!(
            core::str::from_utf8(pool.as_bytes())
                .unwrap()
                .split(['\n', '\t'])
                .all(|label| !label.chars().any(char::is_control))
        );
    }

    #[test]
    fn the_pool_refuses_what_does_not_fit() {
        let mut pool = Pool::new();
        for menu in 0..MAX_MENUS {
            assert!(pool.menu(if menu == 0 { "A" } else { "B" }));
        }
        assert!(!pool.menu("C"));
        let mut pool = Pool::new();
        pool.menu("A");
        for _ in 0..MAX_ENTRIES {
            assert!(pool.separator());
        }
        assert!(!pool.separator());
        let mut pool = Pool::new();
        pool.menu("A");
        let long = "x".repeat(POOL_CAPACITY);
        assert!(!pool.item(&long, "", true));
        assert_eq!(pool.as_bytes(), b"MA\n");
    }

    #[test]
    fn a_menu_chosen_record_names_its_window_menu_and_entry() {
        let mut record = [0u8; RECORD_SIZE];
        record[0..2].copy_from_slice(&MENU_CHOSEN.to_ne_bytes());
        record[2..4].copy_from_slice(&130u16.to_ne_bytes());
        record[4..8].copy_from_slice(&5000u32.to_ne_bytes());
        record[8..12].copy_from_slice(&1u32.to_ne_bytes());
        record[12..16].copy_from_slice(&2u32.to_ne_bytes());
        assert_eq!(
            Chosen::from_record(&record),
            Some(Chosen {
                window: 130,
                menu: 1,
                entry: 2
            })
        );
        record[0..2].copy_from_slice(&13u16.to_ne_bytes());
        assert_eq!(Chosen::from_record(&record), None);
    }

    #[test]
    fn key_labels_spell_the_modifiers_in_macos_order() {
        assert_eq!(
            key_label(u32::from(b'q'), modifier::COMMAND).as_str(),
            "Cmd+Q"
        );
        assert_eq!(
            key_label(u32::from(b'Q'), modifier::COMMAND).as_str(),
            "Shift+Cmd+Q"
        );
        assert_eq!(
            key_label(
                u32::from(b'k'),
                modifier::COMMAND | modifier::OPTION | modifier::CONTROL
            )
            .as_str(),
            "Ctrl+Alt+Cmd+K"
        );
        assert_eq!(key_label(0x0d, 0).as_str(), "Enter");
        assert_eq!(
            key_label(u32::from(b','), modifier::COMMAND).as_str(),
            "Cmd+,"
        );
        assert_eq!(
            key_label(function::F1 + 4, modifier::SHIFT).as_str(),
            "Shift+F5"
        );
        assert_eq!(key_label(function::F12, 0).as_str(), "F12");
        assert_eq!(key_label(0, modifier::COMMAND).as_str(), "");
    }

    #[test]
    fn a_key_equivalent_answers_its_key_and_exact_modifiers() {
        let command = modifier::COMMAND;
        assert!(answers(u32::from(b'q'), command, u32::from(b'q'), command));
        assert!(!answers(
            u32::from(b'q'),
            command,
            u32::from(b'q'),
            command | modifier::OPTION
        ));
        assert!(!answers(u32::from(b'q'), command, u32::from(b'w'), command));
        assert!(answers(
            u32::from(b'Q'),
            command,
            u32::from(b'Q'),
            command | modifier::SHIFT
        ));
        assert!(!answers(u32::from(b'q'), command, u32::from(b'q'), 0));
        assert!(answers(0x0d, 0, 0x0d, modifier::CAPS_LOCK));
        assert!(!answers(0, command, 0, command));
    }
}
