//! The on-screen keyboard (docs/ux/on-screen-keyboard-v0.5.md, D314): a shell panel the server
//! draws at the bottom of the work area in the phone, tablet and TV forms, which shows when a
//! text field takes the focus by a tap (or OK on a TV) and no keyboard is attached. Its keys go
//! to the focused window as a keyboard's would: the kernel takes [`ScreenKey`]s from
//! [`Desktop::take_screen_keys`] and feeds them through its own typist, so a letter key types
//! the layout's character, a dead accent waits as a dead key's does, and the events carry no
//! mark of where they came from.

use super::form::Form;
use super::paint::Canvas;
use super::protocol::EventSink;
use super::{Action, Desktop, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;
use crate::input::keymap::Modifiers;
use crate::input::layout::{self, DEAD_ACCENTS, Typist};
use alloc::vec::Vec;

/// How long a character key is held before its alternates open, and the layout key's menu.
pub const LONG_PRESS_MS: u32 = 500;
const PANEL_HEIGHT_PHONE: u32 = 260;
const PANEL_HEIGHT_PHONE_LANDSCAPE: u32 = 176;
const PANEL_HEIGHT_TABLET: u32 = 300;
const PANEL_HEIGHT_TV: u32 = 204;
const KEYS_ROW: u32 = 44;
const PHONE_ROW: u32 = 54;
const TABLET_ROW: u32 = 64;
const LANDSCAPE_ROW: u32 = 44;
const PHONE_WIDTH: u32 = 390;
const PHONE_PITCH: u32 = 39;
const CAP_INSET: u32 = 2;
const STRIP_CELL: u32 = 44;
const PREVIEW: (u32, u32) = (47, 44);
const TV_CELL: (u32, u32) = (72, 40);
const TV_LEFT: u32 = 48;
const TV_TOP: u32 = 8;
const TV_COLUMNS: usize = 12;
const TV_FIRST_FOCUS: (usize, usize) = (4, 1);
const LATCH_LINE_INSET: u32 = 4;

/// `keyboard.on_screen`: when the keyboard shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnScreenKeyboardMode {
    /// When a text field is tapped and no keyboard is attached.
    #[default]
    Auto,
    /// When a text field is tapped or clicked, keyboard or not.
    Always,
    /// Never.
    Never,
}

impl OnScreenKeyboardMode {
    /// The setting's values, in this order.
    pub const NAMES: [&'static str; 3] = ["auto", "always", "never"];

    /// The mode named `name`.
    pub fn named(name: &[u8]) -> Option<Self> {
        match name {
            b"auto" => Some(Self::Auto),
            b"always" => Some(Self::Always),
            b"never" => Some(Self::Never),
            _ => None,
        }
    }

    /// The mode numbered `number` in [`Self::NAMES`]' order; `Auto` for any other.
    pub fn from_number(number: u32) -> Self {
        match number {
            1 => Self::Always,
            2 => Self::Never,
            _ => Self::Auto,
        }
    }
}

/// What took a window's text focus (`SET_TEXT_FOCUS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldKind {
    /// No text field has the focus.
    None,
    #[default]
    Text,
    Multiline,
    Masked,
    Number,
    Email,
    Url,
    Search,
    Terminal,
}

impl FieldKind {
    /// The kind numbered `number`, in the order above.
    pub fn from_number(number: u32) -> Self {
        match number {
            0 => Self::None,
            2 => Self::Multiline,
            3 => Self::Masked,
            4 => Self::Number,
            5 => Self::Email,
            6 => Self::Url,
            7 => Self::Search,
            8 => Self::Terminal,
            _ => Self::Text,
        }
    }

    /// Its number, for the request.
    pub fn number(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Text => 1,
            Self::Multiline => 2,
            Self::Masked => 3,
            Self::Number => 4,
            Self::Email => 5,
            Self::Url => 6,
            Self::Search => 7,
            Self::Terminal => 8,
        }
    }
}

/// The word on the Enter key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnterWord {
    #[default]
    Enter,
    Go,
    Search,
    Next,
    Done,
    Send,
}

impl EnterWord {
    /// The word numbered `number`, in the order above.
    pub fn from_number(number: u32) -> Self {
        match number {
            1 => Self::Go,
            2 => Self::Search,
            3 => Self::Next,
            4 => Self::Done,
            5 => Self::Send,
            _ => Self::Enter,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Enter => "Enter",
            Self::Go => "Go",
            Self::Search => "Search",
            Self::Next => "Next",
            Self::Done => "Done",
            Self::Send => "Send",
        }
    }
}

/// Why a field took the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusCause {
    /// A tap or a click in it.
    Tap,
    /// OK on it, on a TV.
    Activate,
    /// The program, Tab, or anything else.
    Other,
}

impl FocusCause {
    /// The cause numbered `number`: 0 tap, 1 activate, anything else other.
    pub fn from_number(number: u32) -> Self {
        match number {
            0 => Self::Tap,
            1 => Self::Activate,
            _ => Self::Other,
        }
    }
}

/// One key the panel typed, for the kernel's typist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKey {
    /// Key `code` pressed and released with `modifiers`.
    Key { code: u16, modifiers: Modifiers },
    /// A character typed with no key of its own.
    Character(char),
    /// A dead accent, given as its spacing character ([`DEAD_ACCENTS`]), waits for the next key.
    Accent(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Letters,
    Numbers,
    Symbols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Shift {
    #[default]
    Off,
    Once,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    Letter { code: u16, plain: char },
    Sign(char),
    Accent(char),
    Shift,
    Backspace,
    Page(Page),
    Layout,
    Space,
    Enter,
    Escape,
    Tab,
    Control,
    Alt,
    Arrow(u16),
    Hide,
}

impl Cap {
    fn is_character(self) -> bool {
        matches!(self, Cap::Letter { .. } | Cap::Sign(_))
    }

    fn repeats(self) -> bool {
        matches!(self, Cap::Backspace | Cap::Arrow(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    zone: Rect,
    cap: Cap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Strip {
    rect: Rect,
    cells: Vec<char>,
    hover: Option<usize>,
    left_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextFocus {
    window: u16,
    kind: FieldKind,
    enter: EnterWord,
}

/// The keyboard's state in the desktop.
#[derive(Debug, Clone, Default)]
pub(super) struct OskState {
    shown: bool,
    mode: OnScreenKeyboardMode,
    keyboard_attached: bool,
    page: Page,
    shift: Shift,
    control: bool,
    alt: bool,
    accent: Option<char>,
    focus: Option<TextFocus>,
    panel: Rect,
    keys: Vec<Key>,
    built_for: Option<(Rect, usize, Page, bool, FieldKind)>,
    pressed: Option<usize>,
    pressed_at_ms: u32,
    repeat_at_ms: Option<u32>,
    strip: Option<Strip>,
    tv_focus: usize,
    output: Vec<ScreenKey>,
    own_keys: u32,
    hidden_by_enter_ms: Option<u32>,
    repeat_delay_ms: u32,
    repeat_interval_ms: u32,
}

fn capital(character: char) -> Option<char> {
    let mut upper = character.to_uppercase();
    let first = upper.next()?;
    (upper.next().is_none() && first != character && u32::from(first) <= 0xff).then_some(first)
}

fn is_letter_key(character: char) -> bool {
    character.is_alphabetic() && character.is_lowercase() && capital(character).is_some()
}

/// The `Letters` page's three rows for layout `number`: the letter keys of codes 16 to 27, 30
/// to 40, and 86, 44 to 53 and 43, then a number-row letter no accent composes (`ß`).
fn letter_rows(number: usize) -> [Vec<(u16, char)>; 3] {
    let layout = layout::layout(number);
    let plain = |code: u16| layout.plain_letter(code).filter(|&character| is_letter_key(character));
    let row = |codes: &mut dyn Iterator<Item = u16>| -> Vec<(u16, char)> {
        codes.filter_map(|code| plain(code).map(|character| (code, character))).collect()
    };
    let first = row(&mut (16..=27));
    let second = row(&mut (30..=40));
    let mut third = row(&mut core::iter::once(86).chain(44..=53).chain(core::iter::once(43)));
    for code in 2..=13 {
        if let Some(character) = layout.plain_letter(code)
            && character.is_alphabetic()
            && !character.is_ascii()
            && capital(character).is_none()
        {
            third.push((code, character));
        }
    }
    [first, second, third]
}

const NUMBERS: [&str; 3] = ["1234567890", "-/:;()$€&@", "?!'\"+=*"];
const SYMBOLS: [&str; 2] = ["[]{}<>^#%_", "\\|~`£¥§¡¿·"];

fn alternates(character: char) -> &'static str {
    match character {
        'a' => "àáâäãåæª",
        'c' => "ç¢©",
        'd' => "ð",
        'e' => "èéêë€",
        'i' => "ìíîï",
        'n' => "ñ",
        'o' => "òóôöõøº",
        'r' => "®",
        's' => "ß§",
        't' => "þ",
        'u' => "ùúûüµ",
        'p' => "¶",
        'y' => "ýÿ",
        '?' => "¿",
        '!' => "¡",
        '"' => "«»",
        '$' => "¢¤",
        '-' => "¬±¯",
        '/' => "÷",
        '*' => "×",
        '1' => "¹¼½",
        '2' => "²",
        '3' => "³¾",
        '0' => "°",
        '.' => "·",
        '|' => "¦",
        _ => "",
    }
}

fn accent_name(accent: char) -> &'static str {
    match accent {
        '´' => "Acute accent",
        '`' => "Grave accent",
        '^' => "Circumflex",
        '¨' => "Diaeresis",
        '~' => "Tilde",
        '°' => "Ring",
        _ => "Cedilla",
    }
}

struct Rows<'a> {
    letters: &'a [Vec<(u16, char)>; 3],
    page: Page,
    kind: FieldKind,
}

impl Rows<'_> {
    fn row(&self, index: usize) -> Vec<Cap> {
        match (self.page, index) {
            (Page::Letters, _) => self.letters[index]
                .iter()
                .map(|&(code, plain)| Cap::Letter { code, plain })
                .collect(),
            (Page::Numbers, _) => NUMBERS[index].chars().map(Cap::Sign).collect(),
            (Page::Symbols, 2) => DEAD_ACCENTS.iter().map(|&accent| Cap::Accent(accent)).collect(),
            (Page::Symbols, _) => SYMBOLS[index].chars().map(Cap::Sign).collect(),
        }
    }

    fn counts(&self) -> (u32, u32, u32) {
        (self.row(0).len() as u32, self.row(1).len() as u32, self.row(2).len() as u32)
    }

    fn shift_key(&self) -> Cap {
        match self.page {
            Page::Letters => Cap::Shift,
            Page::Numbers => Cap::Page(Page::Symbols),
            Page::Symbols => Cap::Page(Page::Numbers),
        }
    }

    fn page_key(&self) -> Cap {
        match self.page {
            Page::Letters => Cap::Page(Page::Numbers),
            _ => Cap::Page(Page::Letters),
        }
    }

    fn comma(&self) -> Cap {
        match self.kind {
            FieldKind::Email => Cap::Sign('@'),
            FieldKind::Url => Cap::Sign('/'),
            _ => Cap::Sign(','),
        }
    }
}

fn push(keys: &mut Vec<Key>, x: u32, y: u32, width: u32, height: u32, cap: Cap) {
    keys.push(Key {
        zone: Rect::new(x, y, width, height),
        cap,
    });
}

fn phone_portrait_keys(panel: Rect, rows: &Rows<'_>, layout_key: bool) -> Vec<Key> {
    let width = panel.width;
    let scaled = |value: u32| value * width / PHONE_WIDTH;
    let mut keys = Vec::new();
    let top = panel.y;
    let edges = [0, 49, 98, 147, 195, 243, 291, 339].map(scaled);
    let caps = [
        Cap::Escape,
        Cap::Tab,
        Cap::Control,
        Cap::Arrow(key::LEFT),
        Cap::Arrow(key::UP),
        Cap::Arrow(key::DOWN),
        Cap::Arrow(key::RIGHT),
        Cap::Hide,
    ];
    for (index, cap) in caps.into_iter().enumerate() {
        let right = edges.get(index + 1).copied().unwrap_or(width);
        push(&mut keys, panel.x + edges[index], top, right - edges[index], KEYS_ROW, cap);
    }
    let (n1, n2, n3) = rows.counts();
    let need = n1.max(n2).max(n3 + 3).max(1);
    let pitch = scaled(PHONE_PITCH).min(width / need).max(1);
    let row_y = |row: u32| top + KEYS_ROW + PHONE_ROW * row;
    for (row, count) in [(0, n1), (1, n2)] {
        let start = (width - count * pitch) / 2;
        for (index, cap) in rows.row(row as usize).into_iter().enumerate() {
            let index = index as u32;
            let left = if index == 0 { 0 } else { start + pitch * index };
            let right = if index + 1 == count { width } else { start + pitch * (index + 1) };
            push(&mut keys, panel.x + left, row_y(row), right - left, PHONE_ROW, cap);
        }
    }
    let start = (width - n3 * pitch) / 2;
    push(&mut keys, panel.x, row_y(2), start, PHONE_ROW, rows.shift_key());
    for (index, cap) in rows.row(2).into_iter().enumerate() {
        push(&mut keys, panel.x + start + pitch * index as u32, row_y(2), pitch, PHONE_ROW, cap);
    }
    let backspace = start + n3 * pitch;
    push(&mut keys, panel.x + backspace, row_y(2), width - backspace, PHONE_ROW, Cap::Backspace);
    let (page, layout, comma, dot, enter) = (scaled(58), scaled(44), scaled(39), scaled(39), scaled(78));
    let mut x = 0;
    push(&mut keys, panel.x, row_y(3), page, PHONE_ROW, rows.page_key());
    x += page;
    if layout_key {
        push(&mut keys, panel.x + x, row_y(3), layout, PHONE_ROW, Cap::Layout);
        x += layout;
    }
    push(&mut keys, panel.x + x, row_y(3), comma, PHONE_ROW, rows.comma());
    x += comma;
    let dot_x = width - enter - dot;
    push(&mut keys, panel.x + x, row_y(3), dot_x.saturating_sub(x), PHONE_ROW, Cap::Space);
    push(&mut keys, panel.x + dot_x, row_y(3), dot, PHONE_ROW, Cap::Sign('.'));
    push(&mut keys, panel.x + width - enter, row_y(3), enter, PHONE_ROW, Cap::Enter);
    keys
}

fn wide_keys(panel: Rect, rows: &Rows<'_>, layout_key: bool, keys_row: bool) -> Vec<Key> {
    let width = panel.width;
    let (n1, n2, n3) = rows.counts();
    let units = (n1 + 1).max(n2 + 1).max(n3 + 3).max(1);
    let unit = width / units;
    let mut keys = Vec::new();
    let row_height = if keys_row { TABLET_ROW } else { LANDSCAPE_ROW };
    let first_row = panel.y + if keys_row { KEYS_ROW } else { 0 };
    let row_y = |row: u32| first_row + row_height * row;
    if keys_row {
        let caps = [Cap::Escape, Cap::Tab, Cap::Control, Cap::Alt];
        for (index, cap) in caps.into_iter().enumerate() {
            push(&mut keys, panel.x + unit * index as u32, panel.y, unit, KEYS_ROW, cap);
        }
        let arrows = [key::LEFT, key::UP, key::DOWN, key::RIGHT];
        for (index, code) in arrows.into_iter().enumerate() {
            let x = unit * (units - 5 + index as u32);
            push(&mut keys, panel.x + x, panel.y, unit, KEYS_ROW, Cap::Arrow(code));
        }
        let hide = unit * (units - 1);
        push(&mut keys, panel.x + hide, panel.y, width - hide, KEYS_ROW, Cap::Hide);
    }
    for (index, cap) in rows.row(0).into_iter().enumerate() {
        push(&mut keys, panel.x + unit * index as u32, row_y(0), unit, row_height, cap);
    }
    let backspace = unit * n1;
    push(&mut keys, panel.x + backspace, row_y(0), width - backspace, row_height, Cap::Backspace);
    let start = (units - 1 - n2.min(units - 1)) * unit / 2;
    for (index, cap) in rows.row(1).into_iter().enumerate() {
        let index = index as u32;
        let left = if index == 0 { 0 } else { start + unit * index };
        push(&mut keys, panel.x + left, row_y(1), start + unit * (index + 1) - left, row_height, cap);
    }
    let enter = start + unit * n2;
    push(&mut keys, panel.x + enter, row_y(1), width - enter, row_height, Cap::Enter);
    push(&mut keys, panel.x, row_y(2), unit, row_height, rows.shift_key());
    let mut x = unit;
    for cap in rows.row(2).into_iter().chain([rows.comma(), Cap::Sign('.')]) {
        push(&mut keys, panel.x + x, row_y(2), unit, row_height, cap);
        x += unit;
    }
    if width - x >= unit {
        push(&mut keys, panel.x + x, row_y(2), width - x, row_height, rows.shift_key());
    }
    push(&mut keys, panel.x, row_y(3), unit, row_height, rows.page_key());
    let mut x = unit;
    if keys_row {
        if layout_key {
            push(&mut keys, panel.x + x, row_y(3), unit, row_height, Cap::Layout);
            x += unit;
        }
        let second = unit * (units - 1);
        push(&mut keys, panel.x + x, row_y(3), second.saturating_sub(x), row_height, Cap::Space);
        push(&mut keys, panel.x + second, row_y(3), width - second, row_height, rows.page_key());
        return keys;
    }
    const SMALL: u32 = 48;
    for cap in [Cap::Escape, Cap::Tab, Cap::Control, Cap::Alt] {
        push(&mut keys, panel.x + x, row_y(3), SMALL, row_height, cap);
        x += SMALL;
    }
    if layout_key {
        push(&mut keys, panel.x + x, row_y(3), unit, row_height, Cap::Layout);
        x += unit;
    }
    let arrows_x = width - 5 * SMALL;
    push(&mut keys, panel.x + x, row_y(3), arrows_x.saturating_sub(x), row_height, Cap::Space);
    for (index, code) in [key::LEFT, key::UP, key::DOWN, key::RIGHT].into_iter().enumerate() {
        push(&mut keys, panel.x + arrows_x + SMALL * index as u32, row_y(3), SMALL, row_height, Cap::Arrow(code));
    }
    push(&mut keys, panel.x + width - SMALL, row_y(3), SMALL, row_height, Cap::Hide);
    keys
}

fn tv_cell(panel: Rect, column: u32, row: u32, columns: u32) -> Rect {
    Rect::new(
        panel.x + TV_LEFT + TV_CELL.0 * column,
        panel.y + TV_TOP + TV_CELL.1 * row,
        TV_CELL.0 * columns,
        TV_CELL.1,
    )
}

fn tv_keys(panel: Rect, rows: &Rows<'_>, layout_key: bool) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut put = |rect: Rect, cap: Cap| keys.push(Key { zone: rect, cap });
    for (column, cap) in rows.row(0).into_iter().take(11).enumerate() {
        put(tv_cell(panel, column as u32, 0, 1), cap);
    }
    put(tv_cell(panel, 11, 0, 1), Cap::Backspace);
    for (column, cap) in rows.row(1).into_iter().take(11).enumerate() {
        put(tv_cell(panel, column as u32, 1, 1), cap);
    }
    put(tv_cell(panel, 11, 1, 1), Cap::Enter);
    put(tv_cell(panel, 0, 2, 1), rows.shift_key());
    for (column, cap) in rows.row(2).into_iter().take(10).enumerate() {
        put(tv_cell(panel, column as u32 + 1, 2, 1), cap);
    }
    put(tv_cell(panel, 11, 2, 1), rows.page_key());
    if layout_key {
        put(tv_cell(panel, 0, 3, 1), Cap::Layout);
    }
    put(tv_cell(panel, 1, 3, 1), rows.comma());
    put(tv_cell(panel, 2, 3, 1), Cap::Sign('.'));
    put(tv_cell(panel, 3, 3, 6), Cap::Space);
    put(tv_cell(panel, 9, 3, 1), Cap::Arrow(key::LEFT));
    put(tv_cell(panel, 10, 3, 1), Cap::Arrow(key::RIGHT));
    put(tv_cell(panel, 11, 3, 1), Cap::Hide);
    keys
}

/// The panel's rect in `form` on a work area `work` on a `screen`.
pub(super) fn panel_rect(form: Form, work: Rect, screen: Rect) -> Option<Rect> {
    let height = match form {
        Form::Phone if screen.height >= screen.width => PANEL_HEIGHT_PHONE,
        Form::Phone => PANEL_HEIGHT_PHONE_LANDSCAPE,
        Form::Tablet => PANEL_HEIGHT_TABLET,
        Form::Tv => PANEL_HEIGHT_TV,
        _ => return None,
    };
    let height = height.min(work.height / 2 + work.height / 4);
    Some(Rect::new(work.x, work.bottom() - height, work.width, height))
}

impl OskState {
    fn modifiers(&self, with_shift: bool) -> Modifiers {
        let mut bits = 0;
        if self.control {
            bits |= Modifiers::CTRL.bits();
        }
        if self.alt {
            bits |= Modifiers::ALT.bits();
        }
        if with_shift && self.shift != Shift::Off {
            bits |= Modifiers::SHIFT.bits();
        }
        Modifiers(bits)
    }

    fn after_character(&mut self) {
        if self.shift == Shift::Once {
            self.shift = Shift::Off;
        }
        self.control = false;
        self.alt = false;
        self.accent = None;
    }
}

impl Desktop {
    /// Whether the on-screen keyboard shows now.
    pub fn on_screen_keyboard_shown(&self) -> bool {
        self.osk.shown
    }

    /// The panel's rect while the keyboard shows, as `wm keyboard` prints it.
    pub fn on_screen_keyboard_rect(&self) -> Option<Rect> {
        self.osk.shown.then_some(self.osk.panel)
    }

    /// Takes the keys the panel typed since the last call, in order, for the kernel's typist.
    /// Their key events come back through [`Desktop::key_typing`], which passes them to the
    /// window without taking them as the remote's presses on the panel.
    pub fn take_screen_keys(&mut self) -> Vec<ScreenKey> {
        let keys = core::mem::take(&mut self.osk.output);
        self.osk.own_keys += keys.iter().filter(|key| matches!(key, ScreenKey::Key { .. })).count() as u32;
        keys
    }

    pub(super) fn take_own_screen_key(&mut self, _code: u16) -> bool {
        let own = self.osk.own_keys > 0;
        self.osk.own_keys = self.osk.own_keys.saturating_sub(1);
        own
    }

    pub(super) fn on_screen_keyboard_holds(&self) -> bool {
        self.osk.shown && (self.osk.pressed.is_some() || self.osk.strip.is_some())
    }

    pub(super) fn close_keyboard_strip(&mut self, damage: &mut Damage) {
        if let Some(strip) = self.osk.strip.take() {
            damage.add(strip.rect);
        }
    }

    pub(super) fn on_screen_keyboard_overlaps(&self, clip: Rect) -> bool {
        if !self.osk.shown {
            return false;
        }
        let panel = self.osk.panel;
        let above = Rect::new(panel.x, panel.y.saturating_sub(TV_CELL.1.max(STRIP_CELL)), panel.width, panel.height + TV_CELL.1.max(STRIP_CELL));
        above.overlaps(&clip)
    }

    /// Sets `keyboard.on_screen` and whether a keyboard is attached (`input.present`); a keyboard
    /// attached in `auto` hides the panel, and `never` hides it always. Sets the held keys'
    /// repeat, the physical keyboard's.
    pub fn set_on_screen_keyboard(
        &mut self,
        mode: OnScreenKeyboardMode,
        keyboard_attached: bool,
        repeat_delay_ms: u32,
        repeat_interval_ms: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let newly_attached = keyboard_attached && !self.osk.keyboard_attached;
        self.osk.mode = mode;
        self.osk.keyboard_attached = keyboard_attached;
        self.osk.repeat_delay_ms = repeat_delay_ms.max(1);
        self.osk.repeat_interval_ms = repeat_interval_ms.max(1);
        let hide = match mode {
            OnScreenKeyboardMode::Never => true,
            OnScreenKeyboardMode::Auto => newly_attached,
            OnScreenKeyboardMode::Always => false,
        };
        if hide && self.osk.shown {
            self.hide_on_screen_keyboard(damage, events);
        }
    }

    fn keyboard_may_show(&self, cause: FocusCause) -> bool {
        let wanted_cause = match self.form {
            Form::Tv => cause == FocusCause::Activate,
            Form::Phone | Form::Tablet => cause == FocusCause::Tap,
            _ => false,
        };
        let mode = match self.osk.mode {
            OnScreenKeyboardMode::Never => false,
            OnScreenKeyboardMode::Always => true,
            OnScreenKeyboardMode::Auto => !self.osk.keyboard_attached,
        };
        wanted_cause && mode
    }

    /// `SET_TEXT_FOCUS` from window `window`: its text focus is now a field of `kind` whose
    /// Enter does `enter`, taken for `cause`. A tap (OK on a TV) shows the keyboard for the
    /// focused window when the mode allows; `FieldKind::None` from the window that had the
    /// keyboard hides it.
    pub fn set_text_focus(
        &mut self,
        window: u16,
        kind: FieldKind,
        enter: EnterWord,
        cause: FocusCause,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let focused = self.focus.and_then(|slot| self.windows[slot].map(|window| window.id));
        if kind == FieldKind::None {
            if self.osk.focus.is_some_and(|focus| focus.window == window) {
                self.osk.focus = None;
                if self.osk.shown {
                    self.hide_on_screen_keyboard(damage, events);
                }
            }
            return;
        }
        if focused != Some(window) {
            return;
        }
        let changed = self.osk.focus.is_none_or(|focus| focus.kind != kind || focus.enter != enter);
        self.osk.focus = Some(TextFocus { window, kind, enter });
        if self.osk.shown && changed {
            self.osk.built_for = None;
            self.rebuild_keys();
            damage.add(self.osk.panel);
        }
        let own_enter = cause == FocusCause::Activate
            && self
                .osk
                .hidden_by_enter_ms
                .is_some_and(|hidden| self.now_ms.wrapping_sub(hidden) < LONG_PRESS_MS);
        if !self.osk.shown && !own_enter && self.keyboard_may_show(cause) {
            self.show_on_screen_keyboard(damage, events);
        }
    }

    /// A tap in window `slot`'s content: the console terminal, which is the server's own and
    /// sends no text focus, takes it as a field of kind `terminal` (brief 5.5).
    pub(super) fn terminal_tapped(&mut self, slot: usize, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(window) = self.windows[slot] else {
            return;
        };
        if window.id != self.terminal_window() {
            return;
        }
        self.set_text_focus(window.id, FieldKind::Terminal, EnterWord::Enter, FocusCause::Tap, damage, events);
    }

    /// Shows the keyboard for the focused window's field, as `wm keyboard show` asks; refused
    /// in the `never` mode, or with no field or form to show it for.
    pub fn show_on_screen_keyboard_by_request(
        &mut self,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), ()> {
        let has_field = self.osk.focus.is_some_and(|focus| {
            self.focus.and_then(|slot| self.windows[slot].map(|window| window.id)) == Some(focus.window)
        });
        let form = matches!(self.form, Form::Phone | Form::Tablet | Form::Tv);
        if self.osk.mode == OnScreenKeyboardMode::Never || !has_field || !form {
            return Err(());
        }
        if !self.osk.shown {
            self.show_on_screen_keyboard(damage, events);
        }
        Ok(())
    }

    fn show_on_screen_keyboard(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let Some(panel) = panel_rect(self.form, self.layout.work_area, self.screen()) else {
            return;
        };
        self.osk.shown = true;
        self.osk.panel = panel;
        self.osk.page = match self.osk.focus.map(|focus| focus.kind) {
            Some(FieldKind::Number) => Page::Numbers,
            _ => Page::Letters,
        };
        self.osk.shift = Shift::Off;
        self.osk.control = false;
        self.osk.alt = false;
        self.osk.strip = None;
        self.osk.pressed = None;
        self.osk.built_for = None;
        self.rebuild_keys();
        self.osk.tv_focus = self.tv_key_at(TV_FIRST_FOCUS.0, TV_FIRST_FOCUS.1).unwrap_or(0);
        self.layout.work_area.height -= panel.height;
        self.refit_for_keyboard(damage, events);
        damage.add(self.layout.work_area);
        damage.add(panel);
    }

    /// Hides the keyboard: `Back`, its `Hide keyboard` key, `wm keyboard hide`, the field
    /// losing the focus, a sheet or the home screen opening. The windows get their size back.
    pub fn hide_on_screen_keyboard(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if !self.osk.shown {
            return;
        }
        let panel = self.osk.panel;
        self.osk.shown = false;
        self.osk.strip = None;
        self.osk.pressed = None;
        self.osk.repeat_at_ms = None;
        self.layout.work_area.height += panel.height;
        self.refit_for_keyboard(damage, events);
        damage.add(self.layout.work_area);
        damage.add(panel);
    }

    fn refit_for_keyboard(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        for slot in 0..self.windows.len() {
            let Some(window) = self.windows[slot] else {
                continue;
            };
            if window.shown && !window.minimised && self.fills_form() {
                self.fit_to_phone(slot, damage, events);
            }
        }
    }

    fn layout_number(&self) -> (usize, bool) {
        let number = self.keyboard_layout_in_force().map_or(layout::US_LAYOUT, |choice| usize::from(choice.number));
        let more = self.keyboard_layout_count() > 1;
        (number, more)
    }

    fn rebuild_keys(&mut self) {
        let (number, layout_key) = self.layout_number();
        let kind = self.osk.focus.map_or(FieldKind::Text, |focus| focus.kind);
        let wanted = Some((self.osk.panel, number, self.osk.page, layout_key, kind));
        if self.osk.built_for == wanted {
            return;
        }
        self.osk.built_for = wanted;
        let letters = letter_rows(number);
        let rows = Rows {
            letters: &letters,
            page: self.osk.page,
            kind,
        };
        let panel = self.osk.panel;
        self.osk.keys = match self.form {
            Form::Tv => tv_keys(panel, &rows, layout_key),
            Form::Phone if self.height >= self.width => phone_portrait_keys(panel, &rows, layout_key),
            Form::Phone => wide_keys(panel, &rows, layout_key, false),
            _ => wide_keys(panel, &rows, layout_key, true),
        };
    }

    fn key_at(&self, x: u32, y: u32) -> Option<usize> {
        self.osk.keys.iter().position(|key| contains(key.zone, x, y))
    }

    fn tv_key_at(&self, column: usize, row: usize) -> Option<usize> {
        let cell = tv_cell(self.osk.panel, column as u32, row as u32, 1);
        self.key_at(cell.x + 1, cell.y + 1)
    }

    /// Whether a press at (`x`, `y`) is the keyboard's: on the panel or its alternates.
    pub(super) fn on_screen_keyboard_takes(&self, x: u32, y: u32) -> bool {
        self.osk.shown
            && (contains(self.osk.panel, x, y) || self.osk.strip.as_ref().is_some_and(|strip| contains(strip.rect, x, y)))
    }

    /// A press on the panel or its strip.
    pub(super) fn on_screen_keyboard_press(&mut self, x: u32, y: u32, right: bool, damage: &mut Damage) {
        self.rebuild_keys();
        if let Some(strip) = &mut self.osk.strip {
            damage.add(strip.rect);
            if contains(strip.rect, x, y) {
                strip.hover = Some(((x - strip.rect.x) / STRIP_CELL) as usize);
                return;
            }
            self.osk.strip = None;
            return;
        }
        let Some(index) = self.key_at(x, y) else {
            return;
        };
        let cap = self.osk.keys[index].cap;
        damage.add(self.osk.keys[index].zone);
        if right && cap == Cap::Layout {
            self.open_keyboard_layout_menu(damage);
            return;
        }
        if right && cap.is_character() {
            self.open_strip(index, damage);
            if let Some(strip) = &mut self.osk.strip {
                strip.left_open = true;
            }
            return;
        }
        self.osk.pressed = Some(index);
        self.osk.pressed_at_ms = self.now_ms;
        if cap.repeats() {
            self.act(cap, damage);
            self.osk.repeat_at_ms = Some(self.now_ms.wrapping_add(self.osk.repeat_delay_ms));
        }
        if self.osk.page == Page::Letters && self.form == Form::Phone && cap.is_character() {
            damage.add(self.preview_rect(index));
        }
    }

    /// The finger or the pointer moved while pressed on the panel.
    pub(super) fn on_screen_keyboard_move(&mut self, x: u32, y: u32, damage: &mut Damage) {
        if let Some(strip) = &mut self.osk.strip {
            let hover = contains(strip.rect, x, y).then(|| ((x - strip.rect.x) / STRIP_CELL) as usize);
            if hover != strip.hover {
                strip.hover = hover;
                damage.add(strip.rect);
            }
            return;
        }
        let Some(pressed) = self.osk.pressed else {
            return;
        };
        let cap = self.osk.keys[pressed].cap;
        let under = self.key_at(x, y);
        if cap.repeats() && under != Some(pressed) {
            self.osk.repeat_at_ms = None;
            self.osk.pressed = None;
            damage.add(self.osk.keys[pressed].zone);
            return;
        }
        if cap.is_character()
            && let Some(under) = under
            && under != pressed
            && self.osk.keys[under].cap.is_character()
        {
            damage.add(self.osk.keys[pressed].zone);
            damage.add(self.preview_rect(pressed));
            self.osk.pressed = Some(under);
            self.osk.pressed_at_ms = self.now_ms;
            damage.add(self.osk.keys[under].zone);
            damage.add(self.preview_rect(under));
        }
    }

    /// The finger lifted or the button went up: a character key types the key under it, a
    /// function key acts when lifted in its own zone, an alternate types when chosen.
    pub(super) fn on_screen_keyboard_release(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        if let Some(strip) = self.osk.strip.take() {
            damage.add(strip.rect);
            let chosen = strip.hover.filter(|_| contains(strip.rect, x, y)).and_then(|cell| strip.cells.get(cell).copied());
            match chosen {
                Some(character) => {
                    self.osk.output.push(ScreenKey::Character(character));
                    self.osk.after_character();
                    damage.add(self.osk.panel);
                }
                None => self.osk.strip = Some(Strip { left_open: true, ..strip }),
            }
            if let Some(pressed) = self.osk.pressed.take() {
                damage.add(self.osk.keys[pressed].zone);
            }
            return Action::None;
        }
        let Some(pressed) = self.osk.pressed.take() else {
            return Action::None;
        };
        self.osk.repeat_at_ms = None;
        let key = self.osk.keys[pressed];
        damage.add(key.zone);
        damage.add(self.preview_rect(pressed));
        if key.cap.repeats() {
            return Action::None;
        }
        if key.cap.is_character() {
            if contains(self.osk.panel, x, y) {
                self.act(key.cap, damage);
            }
            return Action::None;
        }
        if !contains(key.zone, x, y) {
            return Action::None;
        }
        match key.cap {
            Cap::Hide => {
                self.hide_on_screen_keyboard(damage, events);
                Action::None
            }
            Cap::Layout => self.next_layout(damage),
            cap => {
                self.act(cap, damage);
                Action::None
            }
        }
    }

    fn next_layout(&mut self, damage: &mut Damage) -> Action {
        let Some(next) = self.next_keyboard_layout() else {
            return Action::None;
        };
        self.osk.accent = None;
        if self.osk.shift == Shift::Once {
            self.osk.shift = Shift::Off;
        }
        self.osk.built_for = None;
        damage.add(self.osk.panel);
        Action::ChooseKeyboardLayout(next)
    }

    /// The time moved on: a held character key opens its alternates after [`LONG_PRESS_MS`], a
    /// held Backspace or arrow repeats; a keyboard whose field lost the focus hides; a work area
    /// laid out again without the panel gets it back.
    pub fn on_screen_keyboard_tick(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        if !self.osk.shown {
            return;
        }
        let focused = self.focus.and_then(|slot| self.windows[slot].map(|window| window.id));
        if self.osk.focus.is_none_or(|focus| Some(focus.window) != focused) {
            return self.hide_on_screen_keyboard(damage, events);
        }
        if self.layout.work_area.bottom() > self.osk.panel.y
            && let Some(panel) = panel_rect(self.form, self.layout.work_area, self.screen())
        {
            self.osk.panel = panel;
            self.layout.work_area.height -= panel.height;
            self.osk.built_for = None;
            self.refit_for_keyboard(damage, events);
            damage.add(self.screen());
        }
        self.rebuild_keys();
        let Some(pressed) = self.osk.pressed else {
            return;
        };
        let cap = self.osk.keys[pressed].cap;
        if let Some(at) = self.osk.repeat_at_ms
            && self.now_ms.wrapping_sub(at) < 1 << 31
        {
            self.act(cap, damage);
            self.osk.repeat_at_ms = Some(at.wrapping_add(self.osk.repeat_interval_ms));
        }
        let held = self.now_ms.wrapping_sub(self.osk.pressed_at_ms) >= LONG_PRESS_MS;
        if held && cap == Cap::Layout {
            self.osk.pressed = None;
            damage.add(self.osk.keys[pressed].zone);
            self.open_keyboard_layout_menu(damage);
            return;
        }
        if held && cap.is_character() && self.osk.strip.is_none() && !self.osk.control && !self.osk.alt {
            self.open_strip(pressed, damage);
        }
    }

    fn open_strip(&mut self, index: usize, damage: &mut Damage) {
        let key = self.osk.keys[index];
        let base = match key.cap {
            Cap::Letter { plain, .. } => plain,
            Cap::Sign(sign) => sign,
            _ => return,
        };
        let shifted = self.osk.shift != Shift::Off;
        let on_page: Vec<char> = self
            .osk
            .keys
            .iter()
            .filter_map(|key| match key.cap {
                Cap::Letter { plain, .. } => Some(plain),
                _ => None,
            })
            .collect();
        let cells: Vec<char> = alternates(base)
            .chars()
            .filter(|character| !on_page.contains(character))
            .filter_map(|character| match shifted {
                false => Some(character),
                true if matches!(character, 'ß' | 'µ' | 'ª' | 'º') => Some(character),
                true if is_letter_key(character) => capital(character),
                true if character.is_alphabetic() => None,
                true => Some(character),
            })
            .collect();
        if cells.is_empty() {
            return;
        }
        let (cell_width, cell_height) = match self.form {
            Form::Tv => TV_CELL,
            _ => (STRIP_CELL, STRIP_CELL),
        };
        let width = cell_width * cells.len() as u32;
        let middle = key.zone.x + key.zone.width / 2;
        let panel = self.osk.panel;
        let left = middle
            .saturating_sub(cell_height / 2)
            .clamp(panel.x, panel.right().saturating_sub(width).max(panel.x));
        let rect = Rect::new(left, key.zone.y.saturating_sub(cell_height), width, cell_height);
        damage.add(rect);
        damage.add(self.preview_rect(index));
        self.osk.strip = Some(Strip {
            rect,
            cells,
            hover: None,
            left_open: false,
        });
    }

    fn preview_rect(&self, index: usize) -> Rect {
        let zone = self.osk.keys[index].zone;
        let x = zone.x.saturating_sub(4).min(self.osk.panel.right().saturating_sub(PREVIEW.0));
        Rect::new(x, zone.y.saturating_sub(PREVIEW.1), PREVIEW.0, PREVIEW.1)
    }

    fn act(&mut self, cap: Cap, damage: &mut Damage) {
        match cap {
            Cap::Letter { code, plain } => {
                self.type_letter(code, plain);
                self.osk.after_character();
                damage.add(self.osk.panel);
            }
            Cap::Sign(character) => {
                let latched = self.osk.control || self.osk.alt;
                let as_key = latched
                    .then(|| u8::try_from(character).ok().and_then(crate::input::keymap::key_for_ascii))
                    .flatten();
                match as_key {
                    Some((code, modifiers)) => self.osk.output.push(ScreenKey::Key {
                        code,
                        modifiers: Modifiers(modifiers.bits() | self.osk.modifiers(false).bits()),
                    }),
                    None => self.osk.output.push(ScreenKey::Character(character)),
                }
                self.osk.after_character();
                damage.add(self.osk.panel);
            }
            Cap::Accent(accent) => {
                self.osk.output.push(ScreenKey::Accent(accent));
                self.osk.accent = Some(accent);
                self.osk.page = Page::Letters;
                self.osk.built_for = None;
                damage.add(self.osk.panel);
            }
            Cap::Shift => {
                self.osk.shift = match self.osk.shift {
                    Shift::Off => Shift::Once,
                    Shift::Once => Shift::Locked,
                    Shift::Locked => Shift::Off,
                };
                damage.add(self.osk.panel);
            }
            Cap::Control => {
                self.osk.control = !self.osk.control;
                damage.add(self.osk.panel);
            }
            Cap::Alt => {
                self.osk.alt = !self.osk.alt;
                damage.add(self.osk.panel);
            }
            Cap::Page(page) => {
                self.osk.page = page;
                if self.osk.shift == Shift::Once {
                    self.osk.shift = Shift::Off;
                }
                self.osk.built_for = None;
                damage.add(self.osk.panel);
            }
            Cap::Space | Cap::Enter | Cap::Escape | Cap::Tab | Cap::Backspace => {
                let code = match cap {
                    Cap::Space => key::SPACE,
                    Cap::Enter => key::ENTER,
                    Cap::Escape => key::ESC,
                    Cap::Tab => key::TAB,
                    _ => key::BACKSPACE,
                };
                self.osk.output.push(ScreenKey::Key {
                    code,
                    modifiers: self.osk.modifiers(false),
                });
                self.osk.after_character();
                damage.add(self.osk.panel);
            }
            Cap::Arrow(code) => {
                self.osk.output.push(ScreenKey::Key {
                    code,
                    modifiers: self.osk.modifiers(true),
                });
                self.osk.control = false;
                self.osk.alt = false;
                damage.add(self.osk.panel);
            }
            Cap::Layout | Cap::Hide => {}
        }
    }

    fn type_letter(&mut self, code: u16, plain: char) {
        let modifiers = self.osk.modifiers(false);
        if self.osk.shift == Shift::Off {
            self.osk.output.push(ScreenKey::Key { code, modifiers });
            return;
        }
        let Some(wanted) = capital(plain) else {
            self.osk.output.push(ScreenKey::Key { code, modifiers });
            return;
        };
        let (number, _) = self.layout_number();
        let mut typist = Typist::new();
        typist.set_layout(number);
        for held in [Modifiers::SHIFT, Modifiers::CAPS_LOCK] {
            if typist.character_of(code, held) == u32::from(wanted) {
                self.osk.output.push(ScreenKey::Key {
                    code,
                    modifiers: Modifiers(modifiers.bits() | held.bits()),
                });
                return;
            }
        }
        self.osk.output.push(ScreenKey::Character(wanted));
    }

    /// A key while the keyboard shows on a TV: the arrows walk the grid, OK types the focused
    /// key, Menu opens its alternates, Back closes them or hides the keyboard. `None` for a key
    /// the keyboard does not take.
    pub(super) fn on_screen_keyboard_tv_key(
        &mut self,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        if !self.osk.shown || self.form != Form::Tv {
            return None;
        }
        self.rebuild_keys();
        let focus = self.osk.tv_focus.min(self.osk.keys.len().saturating_sub(1));
        let zone = self.osk.keys.get(focus)?.zone;
        let column = ((zone.x - self.osk.panel.x - TV_LEFT) / TV_CELL.0) as usize;
        let row = ((zone.y - self.osk.panel.y - TV_TOP) / TV_CELL.1) as usize;
        let moved = match code {
            key::LEFT => (0..column).rev().find_map(|to| self.tv_key_at(to, row).filter(|&key| key != focus)),
            key::RIGHT => (column + zone.width as usize / TV_CELL.0 as usize..TV_COLUMNS)
                .find_map(|to| self.tv_key_at(to, row).filter(|&key| key != focus)),
            key::UP if row > 0 => (0..=column).rev().find_map(|to| self.tv_key_at(to, row - 1)),
            key::DOWN if row < 3 => (0..=column).rev().find_map(|to| self.tv_key_at(to, row + 1)),
            key::UP | key::DOWN => Some(focus),
            key::ENTER | key::SPACE => {
                let cap = self.osk.keys[focus].cap;
                return Some(match cap {
                    Cap::Hide => {
                        self.hide_on_screen_keyboard(damage, events);
                        Action::None
                    }
                    Cap::Layout => self.next_layout(damage),
                    Cap::Enter => {
                        self.act(cap, damage);
                        let stays = self.osk.focus.is_some_and(|field| {
                            matches!(field.kind, FieldKind::Multiline | FieldKind::Terminal)
                        });
                        if !stays {
                            self.hide_on_screen_keyboard(damage, events);
                            self.osk.hidden_by_enter_ms = Some(self.now_ms);
                        }
                        Action::None
                    }
                    cap => {
                        self.act(cap, damage);
                        Action::None
                    }
                });
            }
            key::COMPOSE => {
                self.open_strip(focus, damage);
                return Some(Action::None);
            }
            key::BACK | key::ESC => {
                match self.osk.strip.take() {
                    Some(strip) => damage.add(strip.rect),
                    None => self.hide_on_screen_keyboard(damage, events),
                }
                return Some(Action::None);
            }
            _ => return None,
        };
        if let Some(next) = moved {
            damage.add(self.osk.keys[focus].zone);
            damage.add(self.osk.keys[next].zone);
            self.osk.tv_focus = next;
        }
        Some(Action::None)
    }

    /// `Back` on a phone or tablet while the keyboard shows: closes the alternates or hides the
    /// keyboard; `false` when it does not show.
    pub(super) fn on_screen_keyboard_back(&mut self, damage: &mut Damage, events: &mut dyn EventSink) -> bool {
        if !self.osk.shown {
            return false;
        }
        match self.osk.strip.take() {
            Some(strip) => damage.add(strip.rect),
            None => self.hide_on_screen_keyboard(damage, events),
        }
        true
    }

    /// Draws the panel, the pressed key's preview and the alternates.
    pub(super) fn draw_on_screen_keyboard(&self, canvas: &mut Canvas<'_>) {
        if !self.osk.shown {
            return;
        }
        let panel = self.osk.panel;
        canvas.fill(panel, colour::BAR);
        canvas.row(panel.y, panel.x, panel.right(), colour::BAR_EDGE);
        let kind = self.osk.focus.map_or(FieldKind::Text, |focus| focus.kind);
        let enter = self.osk.focus.map_or(EnterWord::Enter, |focus| match focus.kind {
            FieldKind::Search => EnterWord::Search,
            FieldKind::Multiline => EnterWord::Enter,
            _ => focus.enter,
        });
        for (index, key) in self.osk.keys.iter().enumerate() {
            let pressed = self.osk.pressed == Some(index);
            let focused = self.form == Form::Tv && self.osk.tv_focus == index;
            self.draw_key(canvas, *key, pressed, focused, enter);
        }
        if let Some(pressed) = self.osk.pressed
            && self.osk.strip.is_none()
            && self.form == Form::Phone
            && self.height >= self.width
            && kind != FieldKind::Masked
            && let Some(character) = self.shown_character(self.osk.keys[pressed].cap)
        {
            let rect = self.preview_rect(pressed);
            canvas.fill(rect, colour::MENU);
            outline(canvas, rect, colour::MENU_BORDER);
            draw_character(canvas, rect, character, colour::TEXT);
        }
        if let Some(strip) = &self.osk.strip {
            canvas.fill(strip.rect, colour::MENU);
            outline(canvas, strip.rect, colour::MENU_BORDER);
            let cell_width = strip.rect.width / strip.cells.len().max(1) as u32;
            for (index, &character) in strip.cells.iter().enumerate() {
                let cell = Rect::new(strip.rect.x + cell_width * index as u32, strip.rect.y, cell_width, strip.rect.height);
                if strip.hover == Some(index) {
                    canvas.fill(inset(cell, 1), colour::SELECTION);
                }
                draw_character(canvas, cell, character, colour::TEXT);
            }
        }
    }

    fn shown_character(&self, cap: Cap) -> Option<char> {
        match cap {
            Cap::Letter { plain, .. } if self.osk.shift != Shift::Off => capital(plain).or(Some(plain)),
            Cap::Letter { plain, .. } => Some(plain),
            Cap::Sign(sign) => Some(sign),
            Cap::Accent(accent) => Some(accent),
            _ => None,
        }
    }

    fn draw_key(&self, canvas: &mut Canvas<'_>, key: Key, pressed: bool, focused: bool, enter: EnterWord) {
        let cap = inset(key.zone, CAP_INSET);
        let latched = match key.cap {
            Cap::Shift => self.osk.shift != Shift::Off,
            Cap::Control => self.osk.control,
            Cap::Alt => self.osk.alt,
            Cap::Accent(accent) => self.osk.accent == Some(accent),
            _ => false,
        };
        let function = !matches!(key.cap, Cap::Letter { .. } | Cap::Sign(_) | Cap::Accent(_) | Cap::Space | Cap::Enter);
        let (fill, border, label) = match key.cap {
            Cap::Enter if pressed => (colour::BUTTON_PRIMARY_HOVER, colour::BUTTON_PRIMARY, colour::ICON_DARK),
            Cap::Enter => (colour::BUTTON_PRIMARY, colour::BUTTON_PRIMARY, colour::ICON_DARK),
            _ if pressed => (colour::BUTTON_PRESSED, if function { colour::MENU_BORDER } else { colour::BUTTON_BORDER }, colour::TEXT),
            _ if latched || focused => (colour::SELECTION, colour::BUTTON_BORDER, colour::TEXT),
            _ if function => (colour::MENU, colour::MENU_BORDER, colour::TEXT),
            _ => (colour::BUTTON, colour::BUTTON_BORDER, colour::TEXT),
        };
        canvas.fill(cap, fill);
        outline(canvas, cap, border);
        if focused {
            outline(canvas, key.zone, colour::FOCUS_RING);
            outline(canvas, inset(key.zone, 1), colour::FOCUS_RING);
        }
        if latched {
            let line_y = cap.bottom().saturating_sub(3);
            for row in line_y..line_y + 2 {
                canvas.row(row, cap.x + LATCH_LINE_INSET, cap.right().saturating_sub(LATCH_LINE_INSET), colour::ACCENT);
            }
        }
        match key.cap {
            Cap::Letter { .. } | Cap::Sign(_) => {
                if let Some(character) = self.shown_character(key.cap) {
                    draw_character(canvas, cap, character, label);
                }
            }
            Cap::Accent(accent) => {
                draw_character(canvas, cap, accent, label);
                let bar_y = cap.y + cap.height / 2 + GLYPH_HEIGHT as u32;
                let bar_x = cap.x + cap.width.saturating_sub(12) / 2;
                for row in bar_y..bar_y + 2 {
                    canvas.row(row, bar_x, bar_x + 12, label);
                }
            }
            Cap::Shift => draw_shift(canvas, cap, self.osk.shift, label),
            Cap::Backspace => draw_backspace(canvas, cap, label),
            Cap::Arrow(code) => draw_triangle(canvas, cap, code, label),
            Cap::Hide => {
                draw_triangle(canvas, cap, key::DOWN, label);
                let bar_y = cap.y + cap.height / 2 + 6;
                let bar_x = cap.x + cap.width.saturating_sub(13) / 2;
                for row in bar_y..bar_y + 2 {
                    canvas.row(row, bar_x, bar_x + 13, label);
                }
            }
            Cap::Space => {
                let (x, y) = (cap.x + cap.width.saturating_sub(24) / 2, cap.y + cap.height / 2);
                canvas.row(y + 3, x, x + 24, label);
                for row in y..y + 3 {
                    canvas.pixel(x, row, label);
                    canvas.pixel(x + 23, row, label);
                }
            }
            Cap::Enter => draw_word(canvas, cap, enter.label(), label),
            Cap::Escape => draw_word(canvas, cap, "Esc", label),
            Cap::Tab => draw_word(canvas, cap, "Tab", label),
            Cap::Control => draw_word(canvas, cap, "Ctrl", label),
            Cap::Alt => draw_word(canvas, cap, "Alt", label),
            Cap::Page(Page::Letters) => draw_word(canvas, cap, "ABC", label),
            Cap::Page(Page::Numbers) => draw_word(canvas, cap, "123", label),
            Cap::Page(Page::Symbols) => draw_word(canvas, cap, "#+=", label),
            Cap::Layout => {
                let code = self.keyboard_layout_in_force().map_or("US", |choice| choice.code);
                draw_word(canvas, cap, code, label);
            }
        }
    }

    /// What `wm keyboard` prints: `hidden`, or `shown` and the panel's rect.
    pub fn on_screen_keyboard_status(&self, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        match self.osk.shown {
            false => out.write_str("hidden"),
            true => {
                let panel = self.osk.panel;
                write!(out, "shown {} {} {} {}", panel.x, panel.y, panel.width, panel.height)
            }
        }
    }

    /// The keys an agent may press while the keyboard shows, by name, for `wm keyboard keys`:
    /// each character key by the character it types now, the named keys by their names.
    pub fn on_screen_keyboard_key_names(&self, name: &mut dyn FnMut(&str, &str)) {
        if !self.osk.shown {
            return;
        }
        let mut buffer = [0u8; 4];
        for key in &self.osk.keys {
            let (label, value): (&str, &str) = match key.cap {
                Cap::Letter { .. } | Cap::Sign(_) => match self.shown_character(key.cap) {
                    Some(character) => (character.encode_utf8(&mut buffer), ""),
                    None => continue,
                },
                Cap::Accent(accent) => (accent_name(accent), if self.osk.accent == Some(accent) { "pending" } else { "" }),
                Cap::Shift => ("Shift", match self.osk.shift {
                    Shift::Off => "off",
                    Shift::Once => "once",
                    Shift::Locked => "locked",
                }),
                Cap::Control => ("Control", if self.osk.control { "latched" } else { "off" }),
                Cap::Alt => ("Alt", if self.osk.alt { "latched" } else { "off" }),
                Cap::Backspace => ("Backspace", ""),
                Cap::Space => ("Space", ""),
                Cap::Enter => ("Enter", ""),
                Cap::Escape => ("Escape", ""),
                Cap::Tab => ("Tab", ""),
                Cap::Arrow(key::LEFT) => ("Left", ""),
                Cap::Arrow(key::UP) => ("Up", ""),
                Cap::Arrow(key::DOWN) => ("Down", ""),
                Cap::Arrow(_) => ("Right", ""),
                Cap::Hide => ("Hide keyboard", ""),
                Cap::Page(Page::Letters) => ("Letters", ""),
                Cap::Page(Page::Numbers) => ("Numbers", ""),
                Cap::Page(Page::Symbols) => ("Symbols", ""),
                Cap::Layout => ("Keyboard layout", self.keyboard_layout_in_force().map_or("", |choice| choice.name)),
            };
            name(label, value);
        }
    }
}

fn inset(rect: Rect, by: u32) -> Rect {
    Rect::new(rect.x + by, rect.y + by, rect.width.saturating_sub(2 * by), rect.height.saturating_sub(2 * by))
}

fn outline(canvas: &mut Canvas<'_>, rect: Rect, colour: u32) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    canvas.row(rect.y, rect.x, rect.right(), colour);
    canvas.row(rect.bottom() - 1, rect.x, rect.right(), colour);
    for y in rect.y..rect.bottom() {
        canvas.pixel(rect.x, y, colour);
        canvas.pixel(rect.right() - 1, y, colour);
    }
}

fn draw_character(canvas: &mut Canvas<'_>, cell: Rect, character: char, colour: u32) {
    let (width, height) = (2 * GLYPH_WIDTH as u32, 2 * GLYPH_HEIGHT as u32);
    let x = cell.x + cell.width.saturating_sub(width) / 2;
    let y = cell.y + cell.height.saturating_sub(height) / 2;
    canvas.character_doubled(x, y, character, colour);
}

fn draw_word(canvas: &mut Canvas<'_>, cell: Rect, word: &str, colour: u32) {
    let width = GLYPH_WIDTH as u32 * word.chars().count() as u32;
    let x = cell.x + cell.width.saturating_sub(width) / 2;
    let y = cell.y + cell.height.saturating_sub(GLYPH_HEIGHT as u32) / 2;
    canvas.text(x, y, word, colour);
}

fn draw_triangle(canvas: &mut Canvas<'_>, cell: Rect, code: u16, colour: u32) {
    let (cx, cy) = (cell.x + cell.width / 2, cell.y + cell.height / 2);
    for step in 0..7u32 {
        let half = step;
        match code {
            key::UP => canvas.row(cy - 3 + step, cx - half, cx + half + 1, colour),
            key::DOWN => canvas.row(cy + 3 - step, cx - half, cx + half + 1, colour),
            key::LEFT => {
                for y in cy - half..=cy + half {
                    canvas.pixel(cx - 3 + step, y, colour);
                }
            }
            _ => {
                for y in cy - half..=cy + half {
                    canvas.pixel(cx + 3 - step, y, colour);
                }
            }
        }
    }
}

fn draw_shift(canvas: &mut Canvas<'_>, cell: Rect, state: Shift, colour: u32) {
    let (cx, top) = (cell.x + cell.width / 2, cell.y + cell.height.saturating_sub(17) / 2);
    for row in 0..7u32 {
        let half = row + 1;
        match state {
            Shift::Off => {
                canvas.pixel(cx - half, top + row, colour);
                canvas.pixel(cx + half - 1, top + row, colour);
            }
            _ => canvas.row(top + row, cx - half, cx + half, colour),
        }
    }
    for row in 7..13u32 {
        match state {
            Shift::Off => {
                canvas.pixel(cx - 3, top + row, colour);
                canvas.pixel(cx + 2, top + row, colour);
            }
            _ => canvas.row(top + row, cx - 3, cx + 3, colour),
        }
    }
    if state == Shift::Off {
        canvas.row(top + 12, cx - 3, cx + 3, colour);
    }
    if state == Shift::Locked {
        for row in 15..17u32 {
            canvas.row(top + row, cx - 7, cx + 7, colour);
        }
    }
}

fn draw_backspace(canvas: &mut Canvas<'_>, cell: Rect, colour: u32) {
    let (left, top) = (cell.x + cell.width.saturating_sub(20) / 2, cell.y + cell.height.saturating_sub(12) / 2);
    let body = left + 6;
    canvas.row(top, body, body + 14, colour);
    canvas.row(top + 11, body, body + 14, colour);
    for row in 0..12u32 {
        canvas.pixel(body + 13, top + row, colour);
        let depth = if row < 6 { row } else { 11 - row };
        canvas.pixel(body - depth, top + row, colour);
    }
    for step in 0..6u32 {
        canvas.pixel(body + 4 + step, top + 3 + step, colour);
        canvas.pixel(body + 9 - step, top + 3 + step, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(number: usize, page: Page) -> Vec<Key> {
        let letters = letter_rows(number);
        let rows = Rows {
            letters: &letters,
            page,
            kind: FieldKind::Text,
        };
        phone_portrait_keys(Rect::new(0, 536, 390, 260), &rows, false)
    }

    fn zone_of(keys: &[Key], cap: Cap) -> Rect {
        keys.iter().find(|key| key.cap == cap).unwrap().zone
    }

    #[test]
    fn the_us_letters_on_a_phone_are_the_design_s() {
        let keys = rows(layout::US_LAYOUT, Page::Letters);
        assert_eq!(zone_of(&keys, Cap::Letter { code: 16, plain: 'q' }), Rect::new(0, 580, 39, 54));
        assert_eq!(zone_of(&keys, Cap::Letter { code: 30, plain: 'a' }), Rect::new(0, 634, 58, 54));
        assert_eq!(zone_of(&keys, Cap::Letter { code: 44, plain: 'z' }), Rect::new(58, 688, 39, 54));
        assert_eq!(zone_of(&keys, Cap::Shift), Rect::new(0, 688, 58, 54));
        assert_eq!(zone_of(&keys, Cap::Backspace), Rect::new(331, 688, 59, 54));
        assert_eq!(zone_of(&keys, Cap::Space), Rect::new(97, 742, 176, 54));
        assert_eq!(zone_of(&keys, Cap::Enter), Rect::new(312, 742, 78, 54));
        assert_eq!(zone_of(&keys, Cap::Hide), Rect::new(339, 536, 51, 44));
    }

    #[test]
    fn german_letters_take_eleven_keys_and_end_in_sharp_s() {
        let letters = letter_rows(layout::named(b"de").unwrap());
        assert_eq!(letters[0].len(), 11);
        assert_eq!(letters[0].last().map(|&(_, character)| character), Some('ü'));
        let third: Vec<char> = letters[2].iter().map(|&(_, character)| character).collect();
        assert_eq!(third, ['y', 'x', 'c', 'v', 'b', 'n', 'm', 'ß']);
        let keys = rows(layout::named(b"de").unwrap(), Page::Letters);
        assert_eq!(zone_of(&keys, Cap::Shift), Rect::new(0, 688, 55, 54));
    }

    #[test]
    fn the_tablet_keys_at_1024_are_the_design_s() {
        let letters = letter_rows(layout::US_LAYOUT);
        let rows = Rows {
            letters: &letters,
            page: Page::Letters,
            kind: FieldKind::Text,
        };
        let keys = wide_keys(Rect::new(0, 404, 1024, 300), &rows, false, true);
        assert_eq!(zone_of(&keys, Cap::Letter { code: 16, plain: 'q' }), Rect::new(0, 448, 93, 64));
        assert_eq!(zone_of(&keys, Cap::Backspace), Rect::new(930, 448, 94, 64));
        assert_eq!(zone_of(&keys, Cap::Enter), Rect::new(883, 512, 141, 64));
        assert_eq!(zone_of(&keys, Cap::Space), Rect::new(93, 640, 837, 64));
        assert_eq!(zone_of(&keys, Cap::Alt), Rect::new(279, 404, 93, 44));
    }

    #[test]
    fn the_landscape_phone_keys_are_the_design_s() {
        let letters = letter_rows(layout::US_LAYOUT);
        let rows = Rows {
            letters: &letters,
            page: Page::Letters,
            kind: FieldKind::Text,
        };
        let keys = wide_keys(Rect::new(0, 214, 796, 176), &rows, false, false);
        assert_eq!(zone_of(&keys, Cap::Letter { code: 16, plain: 'q' }), Rect::new(0, 214, 72, 44));
        assert_eq!(zone_of(&keys, Cap::Enter), Rect::new(684, 258, 112, 44));
        assert_eq!(zone_of(&keys, Cap::Escape), Rect::new(72, 346, 48, 44));
        assert_eq!(zone_of(&keys, Cap::Space), Rect::new(264, 346, 292, 44));
        assert_eq!(zone_of(&keys, Cap::Hide), Rect::new(748, 346, 48, 44));
    }

    #[test]
    fn the_tv_grid_starts_at_its_corner() {
        let letters = letter_rows(layout::US_LAYOUT);
        let rows = Rows {
            letters: &letters,
            page: Page::Letters,
            kind: FieldKind::Text,
        };
        let keys = tv_keys(Rect::new(0, 336, 960, 204), &rows, false);
        assert_eq!(zone_of(&keys, Cap::Letter { code: 16, plain: 'q' }), Rect::new(48, 344, 72, 40));
        assert_eq!(zone_of(&keys, Cap::Letter { code: 34, plain: 'g' }), Rect::new(336, 384, 72, 40));
        assert_eq!(zone_of(&keys, Cap::Space), Rect::new(264, 464, 432, 40));
    }

    #[test]
    fn alternates_leave_out_what_has_no_capital() {
        assert_eq!(capital('é'), Some('É'));
        assert_eq!(capital('ß'), None);
        assert_eq!(capital('ÿ'), None);
        assert!(alternates('e').contains('€'));
    }
}
