//! Settings' Windows pane (D231, docs/ux/windows-registry-v0.3.md): the registry Windows
//! programs on EggOS see, browsed and edited with the words, checks and writes of `windows
//! registry` and the MCP tools `windows_registry_*` (`eggos_windows_registry`): the three roots
//! and their keys in a list the pane flattens, the selected key's values in a table, and panels
//! to edit a value, make a key or a value, delete one, and replace a damaged registry file. The
//! pane reads and writes the registry itself, through the [`Store`] the program gives it, so
//! every write goes through [`Registry`] as the command's does.

use crate::settings_window::CAPACITY;
use crate::storage_pane::{format_panel, shortened};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering as Order;
use eggos_toolkit::cell_store::{CELL_BYTES, CellSlot, CellStore, cut_length, data_of};
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{
    Align, Column, Escape, Kind, Length, Outline, OutlineRow, Place, SortOrder, Table,
};
use eggos_windows_registry::command::{self, VALUE_NOT_SET, name_text};
use eggos_windows_registry::hive::Value;
use eggos_windows_registry::path::{KeyPath, Root, utf8, utf16};
use eggos_windows_registry::registry::{
    DAMAGED_SUFFIX, HiveFile, Registry, Store, StoreError, USER_HIVE_PATH,
};
use eggos_windows_registry::value::{
    EDITABLE, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ, data_text,
    hex_text, number_of, parse_hex, string_data, string_of, strings_data, type_name,
};
use eggos_windows_registry::{
    Error, MAX_DEPTH, MAX_KEY_NAME_UNITS, MAX_VALUE_NAME_UNITS, compare_names, same_name,
};

/// The pane's height: the outline and the table fill what the rows above and below leave.
pub const HEIGHT: i32 = 348;
/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 12 + 9 + 9 + 7;
/// The most keys the list shows below one key.
pub const MAX_SUBKEYS_SHOWN: usize = 1000;
/// The most rows the key list holds.
pub const MAX_KEY_ROWS: usize = 1000;
/// The most values the table holds.
pub const MAX_VALUE_ROWS: usize = 256;
/// The status line of a writable key.
pub const NORMAL_LINE: &str =
    "Enter edits a value, Delete removes it, F5 reads the registry again.";
/// The status line while a root is selected.
pub const ROOT_LINE: &str = "Roots are always there: add a key under one.";
/// The status line before any Windows program saved settings.
pub const NOTHING_YET_LINE: &str =
    "No Windows program has saved settings yet. Keys appear here as programs write them.";
/// The status line under `HKEY_LOCAL_MACHINE` for anyone but root.
pub const MACHINE_WIDE_LINE: &str =
    "HKEY_LOCAL_MACHINE is machine-wide: only an administrator can change it.";
/// The status line under `HKEY_LOCAL_MACHINE` for root.
pub const EVERY_USER_LINE: &str = "Changes here apply to every user of this machine.";
/// The status line under `HKEY_CLASSES_ROOT`.
pub const CLASSES_LINE: &str = "Changes here are saved in HKEY_CURRENT_USER\\Software\\Classes.";
/// The status line while the machine's hive cannot be read.
pub const MACHINE_DAMAGED_LINE: &str = "The machine's registry file is damaged: windows registry reset --machine replaces it (administrator).";
/// The status line while the user's hive cannot be read.
pub const USER_DAMAGED_LINE: &str = "Your registry file is damaged. Windows programs start with an empty registry until it is replaced.";
/// The status line after the user's hive was replaced.
pub const REPLACED_LINE: &str = "Replaced the registry file. It is empty.";
/// What the table shows for a key that cannot be read.
pub const UNREADABLE: &str = "Could not be read.";

const F5: u16 = 63;
const PADDING: i32 = 16;
const HEADING_TOP: i32 = 16;
const PATH_FIELD: (i32, i32, i32, i32) = (16, 64, 232, 28);
const COPY_PATH: (i32, i32) = (256, 64);
const REFRESH: (i32, i32) = (352, 64);
const LISTS_TOP: i32 = 100;
const LISTS_ROOM: i32 = 176;
const KEYS_X_WIDTH: (i32, i32) = (16, 176);
const VALUES_X_WIDTH: (i32, i32) = (200, 248);
const BUTTONS_FROM_BOTTOM: i32 = 68;
const STATUS_FROM_BOTTOM: i32 = 32;
const BUTTON: (i32, i32) = (96, 28);
const BUTTON_XS: [i32; 4] = [16, 120, 224, 328];
const REPLACE_WIDTH: i32 = 136;
const LINE_WIDTH: i32 = 432;
const PANEL_X: i32 = 32;
const PANEL_WIDTH: i32 = 400;
const PANEL_TEXT_WIDTH: i32 = 368;
const PANEL_LINES: [i32; 3] = [16, 36, 56];
const PANEL_FIELD_TOP: i32 = 64;
const PANEL_SECOND_ROW: i32 = 100;
const PANEL_LOW_LINE: i32 = 136;
const PANEL_CANCEL_X: i32 = 192;
const PANEL_PRIMARY_X: i32 = 288;
const PANEL_BUTTONS_FROM_BOTTOM: i32 = 44;
const NUMBER_PANEL: i32 = 208;
const TEXT_PANEL: i32 = 172;
const KEY_PANEL: i32 = 172;
const VALUE_PANEL: i32 = 208;
const CONFIRM_PANEL: i32 = 152;
const FIELD_BYTES: usize = 255;
const CUT_FIELD_BYTES: usize = 252;
const INDENT: &str = "    ";
const COLLAPSED: &str = "+ ";
const EXPANDED: &str = "- ";
const LEAF: &str = "  ";
const DEEPEST_INDENTED: usize = 6;
const DEEPER: &str = "\u{2026} ";
const SHOWN_DATA_BYTES: usize = 256;
const VALUE_CELL_COLUMNS: usize = 3;
const SLOTS: usize = if cfg!(test) { 64 } else { 2 };
const KEY_NAME: usize = 0;
const KEY_SHAPE: usize = 1;
const KEY_CELL_COLUMNS: usize = 2;
const SHAPE_EXPANDABLE: u8 = b'e';
const SHAPE_EXPANDED: u8 = b'o';
const SHAPE_LEAF: u8 = b'-';
const UP_ROW: &str = "..";
const NO_SUBKEYS: &str = "No subkeys.";
const MOST_PATH_LEVELS: usize = 64;
const PHONE_WIDTH: i32 = 448;
const PHONE_COPY_TOP: i32 = 100;
const PHONE_KEYS_TOP: i32 = 148;
const PHONE_ROW: i32 = 40;
const PHONE_LIST_ROWS: i32 = 8;
const PHONE_GAP: i32 = 8;
const PHONE_KEYS_HEIGHT: i32 = PHONE_LIST_ROWS * 24;
const PHONE_VALUES_HEIGHT: i32 = 24 + PHONE_LIST_ROWS * 24;
const PHONE_BUTTONS_TOP: i32 =
    PHONE_KEYS_TOP + PHONE_KEYS_HEIGHT + PHONE_GAP + PHONE_VALUES_HEIGHT + PADDING;
/// The pane's height on a phone: its rows one under another, scrolled by the window.
pub const PHONE_HEIGHT: i32 = PHONE_BUTTONS_TOP + 2 * (PHONE_ROW + PHONE_GAP) + 48;
const VALUE_COLUMNS: [Column; 3] = [
    Column {
        min_width: 64,
        ..Column::new("Name", 96, Align::Start)
    },
    Column {
        min_width: 64,
        ..Column::new("Type", 104, Align::Start)
    },
    Column {
        min_width: 64,
        ..Column::new("Data", 200, Align::Start)
    },
];
const TYPE_NAMES: [&str; 6] = [
    "REG_SZ",
    "REG_EXPAND_SZ",
    "REG_MULTI_SZ",
    "REG_DWORD",
    "REG_QWORD",
    "REG_BINARY",
];
const BASES: [&str; 2] = ["Hexadecimal", "Decimal"];
const KEY_PLACEHOLDER: &str = "Key name";
const VALUE_PLACEHOLDER: &str = "Value name";
const DATA_PLACEHOLDER: &str = "Value data";
const CLASSES: [&str; 2] = ["Software", "Classes"];

static KEY_CELLS: CellStore<SLOTS, MAX_KEY_ROWS, KEY_CELL_COLUMNS> = CellStore::new();
static VALUE_CELLS: CellStore<SLOTS, MAX_VALUE_ROWS, VALUE_CELL_COLUMNS> = CellStore::new();

type KeySlot = CellSlot<'static, SLOTS, MAX_KEY_ROWS, KEY_CELL_COLUMNS>;
type ValueSlot = CellSlot<'static, SLOTS, MAX_VALUE_ROWS, VALUE_CELL_COLUMNS>;

fn key_shape(data: u32, row: usize) -> (u8, u8) {
    let mut shape = Text::<64>::new();
    KEY_CELLS.write(data, row, KEY_SHAPE, &mut shape);
    let bytes = shape.as_str().as_bytes();
    let digits = bytes.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let depth = bytes[..digits]
        .iter()
        .fold(0u32, |sum, digit| sum * 10 + u32::from(digit - b'0'));
    let state = bytes.get(digits).copied().unwrap_or(SHAPE_LEAF);
    (depth.min(u32::from(u8::MAX)) as u8, state)
}

fn key_row(data: u32, row: usize, out: &mut OutlineRow) {
    KEY_CELLS.write(data, row, KEY_NAME, &mut out.text);
    let (depth, state) = key_shape(data, row);
    out.depth = depth;
    out.expandable = state != SHAPE_LEAF;
    out.expanded = state == SHAPE_EXPANDED;
}

fn key_path(data: u32, row: usize, out: &mut Text<256>) {
    let mut chain = [0usize; MOST_PATH_LEVELS];
    let mut length = 0;
    let (mut depth, _) = key_shape(data, row);
    chain[0] = row;
    length += 1;
    let mut above = row;
    while depth > 0 && above > 0 && length < MOST_PATH_LEVELS {
        above -= 1;
        let (level, _) = key_shape(data, above);
        if level < depth {
            chain[length] = above;
            length += 1;
            depth = level;
        }
    }
    for (index, &held) in chain[..length].iter().rev().enumerate() {
        if index > 0 && !out.push_str("\\") {
            return;
        }
        let mut name = Text::<64>::new();
        KEY_CELLS.write(data, held, KEY_NAME, &mut name);
        if !out.push_str(name.as_str()) {
            return;
        }
    }
}

fn value_cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    VALUE_CELLS.write(data, row, column, out);
}

/// The files the pane's registry reads and writes, with a generation the pane bumps to read
/// both hives again on `Refresh` even when their files' stamps did not change.
pub struct RegistryStore {
    files: Box<dyn Store>,
    generation: u64,
}

impl RegistryStore {
    /// The registry's files through `files`.
    pub fn new(files: Box<dyn Store>) -> Self {
        Self {
            files,
            generation: 0,
        }
    }
}

impl core::fmt::Debug for RegistryStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("RegistryStore")
    }
}

impl Store for RegistryStore {
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError> {
        self.files.read(file)
    }

    fn stamp(&mut self, file: HiveFile) -> Option<u64> {
        self.files
            .stamp(file)
            .map(|stamp| stamp.wrapping_add(self.generation))
    }

    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError> {
        self.files.write_temporary(file, bytes)
    }

    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError> {
        self.files.replace_with_temporary(file)
    }

    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError> {
        self.files.set_aside(file)
    }

    fn lock(&mut self, file: HiveFile) -> Result<(), StoreError> {
        self.files.lock(file)
    }

    fn unlock(&mut self, file: HiveFile) {
        self.files.unlock(file);
    }

    fn user_id(&mut self) -> u32 {
        self.files.user_id()
    }

    fn now(&mut self) -> u64 {
        self.files.now()
    }
}

/// What the pane did with a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// It was not for the pane.
    NotMine,
    /// The pane acted on it; with `true` the program puts [`WindowsPane::to_copy`] on the
    /// clipboard.
    Done(bool),
}

#[derive(Debug, Clone, Copy)]
struct Widgets {
    path: WidgetId,
    copy: WidgetId,
    refresh: WidgetId,
    keys: WidgetId,
    values: WidgetId,
    new_key: WidgetId,
    new_value: WidgetId,
    edit: WidgetId,
    delete: WidgetId,
    replace: WidgetId,
    status: WidgetId,
    edit_panel: WidgetId,
    edit_title: WidgetId,
    edit_sub: WidgetId,
    edit_field: WidgetId,
    edit_bases: WidgetId,
    edit_line: WidgetId,
    edit_cancel: WidgetId,
    edit_save: WidgetId,
    name_panel: WidgetId,
    name_title: WidgetId,
    name_sub: WidgetId,
    name_field: WidgetId,
    name_type: WidgetId,
    name_line: WidgetId,
    name_cancel: WidgetId,
    name_create: WidgetId,
    confirm_panel: WidgetId,
    confirm_title: WidgetId,
    confirm_detail: WidgetId,
    confirm_note: WidgetId,
    confirm_cancel: WidgetId,
    confirm_primary: WidgetId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyRow {
    path: KeyPath,
    depth: usize,
    expandable: bool,
    expanded: bool,
    more: usize,
    up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValueRow {
    value: Value,
    set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Panel {
    Closed,
    Edit {
        value: Value,
        created: bool,
        hexadecimal: bool,
        editable: bool,
    },
    NewKey,
    NewValue,
    DeleteKey(KeyPath),
    DeleteValue(Vec<u16>),
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    Keys,
    Values,
    Button(WidgetId),
}

/// The Windows pane's widgets, the registry it shows, and what is selected.
#[derive(Debug)]
pub struct WindowsPane {
    widgets: Widgets,
    registry: Option<Registry<RegistryStore>>,
    rows: Vec<KeyRow>,
    expanded: Vec<KeyPath>,
    selected: KeyPath,
    drill_down: bool,
    page: Option<KeyPath>,
    values: Vec<ValueRow>,
    sort: (usize, SortOrder),
    values_focused: bool,
    panel: Panel,
    opener: Opener,
    said: Option<(String, Token)>,
    to_copy: String,
    user_damaged: bool,
    machine_damaged: bool,
    key_slot: Option<KeySlot>,
    value_slot: Option<ValueSlot>,
    default_button: WidgetId,
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn at(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
}

fn status_place(x: i32, y: i32, width: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Content)
}

fn line(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, x: i32, y: i32, width: i32) -> WidgetId {
    add(
        tree,
        parent,
        Place::at(x, y, Length::Px(width), Length::Content),
        Kind::label("", Token::Text),
    )
}

fn button(
    tree: &mut WidgetTree<CAPACITY>,
    parent: WidgetId,
    x: i32,
    y: i32,
    text: &str,
) -> WidgetId {
    add(
        tree,
        parent,
        at(x, y, BUTTON.0, BUTTON.1),
        Kind::button(text, false),
    )
}

fn show_line(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, words: &str, ink: Token) {
    tree.set_text(id, shortened(words).as_str());
    let inked = matches!(tree.kind(id), Some(Kind::Label { colour, .. }) if *colour == ink);
    if !inked {
        tree.change(id, |kind| {
            if let Kind::Label { colour, .. } = kind {
                *colour = ink;
            }
        });
    }
}

fn set_placeholder(tree: &mut WidgetTree<CAPACITY>, id: WidgetId, placeholder: &'static str) {
    tree.change(id, |kind| {
        if let Kind::TextField(field) = kind {
            field.placeholder = placeholder;
        }
    });
}

fn field_text(text: &str) -> String {
    if text.len() <= FIELD_BYTES {
        return String::from(text);
    }
    let mut start = text.len() - CUT_FIELD_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &text[start..])
}

fn cell_data(row: &ValueRow) -> String {
    match row.set {
        true => shown_data(row.value.kind, &row.value.data),
        false => String::from(VALUE_NOT_SET),
    }
}

fn shown_data(kind: u32, data: &[u8]) -> String {
    let mut text = data_text(kind, &data[..data.len().min(SHOWN_DATA_BYTES)]);
    text.truncate(cut_length(&text, CELL_BYTES));
    text
}

fn grouped(number: usize) -> String {
    let digits = format!("{number}");
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

fn parent_of(path: &KeyPath) -> Option<KeyPath> {
    let mut parent = path.clone();
    parent.names.pop()?;
    Some(parent)
}

fn is_under(path: &KeyPath, above: &KeyPath) -> bool {
    path.root == above.root
        && path.names.len() > above.names.len()
        && path
            .names
            .iter()
            .zip(&above.names)
            .all(|(name, other)| same_name(name, other))
}

fn same_key(one: &KeyPath, other: &KeyPath) -> bool {
    one.root == other.root
        && one.names.len() == other.names.len()
        && one
            .names
            .iter()
            .zip(&other.names)
            .all(|(name, other)| same_name(name, other))
}

fn counted(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

fn key_contents(subkeys: usize, values: usize) -> String {
    match (subkeys, values) {
        (0, 0) => String::from("It is empty."),
        (0, values) => format!("With its {}.", counted(values, "value", "values")),
        (subkeys, 0) => format!("With its {}.", counted(subkeys, "subkey", "subkeys")),
        (subkeys, values) => format!(
            "With its {} and {}.",
            counted(subkeys, "subkey", "subkeys"),
            counted(values, "value", "values")
        ),
    }
}

fn number_limit(kind: u32) -> u64 {
    match kind {
        REG_DWORD => u64::from(u32::MAX),
        _ => u64::MAX,
    }
}

fn not_a_number(kind: u32) -> String {
    let limit = number_limit(kind);
    format!(
        "Not a number. A {} is 0 to {limit} (0 to {limit:x} in hex).",
        type_name(kind)
    )
}

fn parse_typed_number(text: &str, hexadecimal: bool) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0);
    }
    match hexadecimal {
        true => {
            let digits = text
                .strip_prefix("0x")
                .or_else(|| text.strip_prefix("0X"))
                .unwrap_or(text);
            u64::from_str_radix(digits, 16).ok()
        }
        false if text.bytes().all(|byte| byte.is_ascii_digit()) => text.parse().ok(),
        false => None,
    }
}

fn parse_typed_binary(text: &str) -> Result<Vec<u8>, &'static str> {
    let mut digits = String::new();
    for word in text.split_whitespace() {
        let word = word
            .strip_prefix("0x")
            .or_else(|| word.strip_prefix("0X"))
            .unwrap_or(word);
        if !word.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Binary data is pairs of hex digits, such as 4a 00 ff.");
        }
        digits.push_str(word);
    }
    if !digits.len().is_multiple_of(2) {
        return Err("Binary data has an odd number of hex digits.");
    }
    parse_hex(&digits).ok_or("Binary data is pairs of hex digits, such as 4a 00 ff.")
}

fn empty_data(kind: u32) -> Vec<u8> {
    match kind {
        REG_SZ | REG_EXPAND_SZ => string_data(""),
        REG_MULTI_SZ => strings_data(&[]),
        REG_DWORD => alloc::vec![0; 4],
        REG_QWORD => alloc::vec![0; 8],
        _ => Vec::new(),
    }
}

fn value_shown(value: &Value) -> String {
    match value.name.is_empty() {
        true => String::from("the (Default) value"),
        false => utf8(&value.name),
    }
}

impl WindowsPane {
    /// Builds the pane in `pane`, a container of the window's [`HEIGHT`] tall;
    /// `default_button` is the window's default button, given back when a panel closes.
    pub fn build(
        tree: &mut WidgetTree<CAPACITY>,
        pane: WidgetId,
        default_button: WidgetId,
    ) -> Self {
        add(
            tree,
            pane,
            Place::at(PADDING, HEADING_TOP, Length::Content, Length::Content),
            Kind::heading("Windows"),
        );
        let path = add(
            tree,
            pane,
            at(PATH_FIELD.0, PATH_FIELD.1, PATH_FIELD.2, PATH_FIELD.3),
            Kind::text_field("Key path", Escape::Revert),
        );
        let copy = button(tree, pane, COPY_PATH.0, COPY_PATH.1, "Copy path");
        let refresh = button(tree, pane, REFRESH.0, REFRESH.1, "Refresh");
        let lists_height = HEIGHT - LISTS_ROOM;
        let mut keys_outline = Outline::new("Keys", 0, key_row, 0);
        keys_outline.empty_text = UNREADABLE;
        keys_outline.path = Some(key_path);
        let keys = add(
            tree,
            pane,
            at(KEYS_X_WIDTH.0, LISTS_TOP, KEYS_X_WIDTH.1, lists_height),
            Kind::Outline(keys_outline),
        );
        let mut values_table = Table::new("Values", &VALUE_COLUMNS, 0, value_cell, 0);
        values_table.empty_text = UNREADABLE;
        values_table.sort = Some((0, SortOrder::Ascending));
        let values = add(
            tree,
            pane,
            at(VALUES_X_WIDTH.0, LISTS_TOP, VALUES_X_WIDTH.1, lists_height),
            Kind::Table(values_table),
        );
        let buttons_top = HEIGHT - BUTTONS_FROM_BOTTOM;
        let new_key = button(tree, pane, BUTTON_XS[0], buttons_top, "New key...");
        let new_value = button(tree, pane, BUTTON_XS[1], buttons_top, "New value...");
        let edit = button(tree, pane, BUTTON_XS[2], buttons_top, "Edit...");
        let delete = button(tree, pane, BUTTON_XS[3], buttons_top, "Delete...");
        let replace = add(
            tree,
            pane,
            at(BUTTON_XS[0], buttons_top, REPLACE_WIDTH, BUTTON.1),
            Kind::button("Replace file...", false),
        );
        let status = line(tree, pane, PADDING, HEIGHT - STATUS_FROM_BOTTOM, LINE_WIDTH);

        let panel = |tree: &mut WidgetTree<CAPACITY>, height: i32| {
            let panel = add(
                tree,
                pane,
                at(PANEL_X, (HEIGHT - height) / 2, PANEL_WIDTH, height),
                Kind::Panel {
                    fill: None,
                    border_right: None,
                },
            );
            add(
                tree,
                panel,
                Place::fill(),
                Kind::Custom {
                    painter: format_panel,
                    data: 0,
                },
            );
            panel
        };
        let panel_line = |tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, y: i32| {
            line(tree, parent, PADDING, y, PANEL_TEXT_WIDTH)
        };
        let field_place = at(PADDING, PANEL_FIELD_TOP, PANEL_TEXT_WIDTH, BUTTON.1);

        let edit_panel = panel(tree, NUMBER_PANEL);
        let edit_title = panel_line(tree, edit_panel, PANEL_LINES[0]);
        let edit_sub = panel_line(tree, edit_panel, PANEL_LINES[1]);
        let edit_field = add(
            tree,
            edit_panel,
            field_place,
            Kind::text_field(DATA_PLACEHOLDER, Escape::Clear),
        );
        let edit_bases = add(
            tree,
            edit_panel,
            Place::at(PADDING, PANEL_SECOND_ROW, Length::Content, Length::Content),
            Kind::radios(&BASES, 0, true),
        );
        let edit_line = panel_line(tree, edit_panel, PANEL_LOW_LINE);
        let edit_cancel = button(
            tree,
            edit_panel,
            PANEL_CANCEL_X,
            NUMBER_PANEL - PANEL_BUTTONS_FROM_BOTTOM,
            "Cancel",
        );
        let edit_save = add(
            tree,
            edit_panel,
            at(
                PANEL_PRIMARY_X,
                NUMBER_PANEL - PANEL_BUTTONS_FROM_BOTTOM,
                BUTTON.0,
                BUTTON.1,
            ),
            Kind::button("Save", true),
        );

        let name_panel = panel(tree, VALUE_PANEL);
        let name_title = panel_line(tree, name_panel, PANEL_LINES[0]);
        let name_sub = panel_line(tree, name_panel, PANEL_LINES[1]);
        let name_field = add(
            tree,
            name_panel,
            field_place,
            Kind::text_field(KEY_PLACEHOLDER, Escape::Clear),
        );
        let name_type = add(
            tree,
            name_panel,
            at(PADDING, PANEL_SECOND_ROW, PANEL_TEXT_WIDTH, BUTTON.1),
            Kind::dropdown("Type", &TYPE_NAMES, 0),
        );
        let name_line = panel_line(tree, name_panel, PANEL_LOW_LINE);
        let name_cancel = button(
            tree,
            name_panel,
            PANEL_CANCEL_X,
            VALUE_PANEL - PANEL_BUTTONS_FROM_BOTTOM,
            "Cancel",
        );
        let name_create = add(
            tree,
            name_panel,
            at(
                PANEL_PRIMARY_X,
                VALUE_PANEL - PANEL_BUTTONS_FROM_BOTTOM,
                BUTTON.0,
                BUTTON.1,
            ),
            Kind::button("Create", true),
        );

        let confirm_panel = panel(tree, CONFIRM_PANEL);
        let confirm_title = panel_line(tree, confirm_panel, PANEL_LINES[0]);
        let confirm_detail = panel_line(tree, confirm_panel, PANEL_LINES[1]);
        let confirm_note = panel_line(tree, confirm_panel, PANEL_LINES[2]);
        let confirm_cancel = button(
            tree,
            confirm_panel,
            PANEL_CANCEL_X,
            CONFIRM_PANEL - PANEL_BUTTONS_FROM_BOTTOM,
            "Cancel",
        );
        let confirm_primary = add(
            tree,
            confirm_panel,
            at(
                PANEL_PRIMARY_X,
                CONFIRM_PANEL - PANEL_BUTTONS_FROM_BOTTOM,
                BUTTON.0,
                BUTTON.1,
            ),
            Kind::button("Delete", true),
        );
        tree.set_person_only(confirm_primary, true);
        for id in [edit_panel, name_panel, confirm_panel, replace] {
            tree.set_shown(id, false);
        }
        Self {
            widgets: Widgets {
                path,
                copy,
                refresh,
                keys,
                values,
                new_key,
                new_value,
                edit,
                delete,
                replace,
                status,
                edit_panel,
                edit_title,
                edit_sub,
                edit_field,
                edit_bases,
                edit_line,
                edit_cancel,
                edit_save,
                name_panel,
                name_title,
                name_sub,
                name_field,
                name_type,
                name_line,
                name_cancel,
                name_create,
                confirm_panel,
                confirm_title,
                confirm_detail,
                confirm_note,
                confirm_cancel,
                confirm_primary,
            },
            registry: None,
            rows: Vec::new(),
            expanded: Vec::new(),
            selected: KeyPath::root(Root::CurrentUser),
            drill_down: false,
            page: None,
            values: Vec::new(),
            sort: (0, SortOrder::Ascending),
            values_focused: false,
            panel: Panel::Closed,
            opener: Opener::Keys,
            said: None,
            to_copy: String::new(),
            user_damaged: false,
            machine_damaged: false,
            key_slot: None,
            value_slot: None,
            default_button,
        }
    }

    /// Reads and writes the registry through `store` from now on.
    pub fn attach(&mut self, tree: &mut WidgetTree<CAPACITY>, store: Box<dyn Store>) {
        self.registry = Some(Registry::new(RegistryStore::new(store)));
        self.reread(tree);
    }

    /// The pane is shown again: an open panel closes and the hives are read again; the
    /// selection stays.
    pub fn reset(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if self.panel != Panel::Closed {
            self.close_panel(tree, None);
        }
        self.said = None;
        self.reread(tree);
    }

    /// The text [`Reply::Done`] with `true` puts on the clipboard.
    pub fn to_copy(&self) -> &str {
        &self.to_copy
    }

    /// The selected key's full path.
    pub fn selected_path(&self) -> String {
        self.selected.text()
    }

    fn reread(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        if let Some(registry) = self.registry.as_mut() {
            let store = registry.store();
            store.generation = store.generation.wrapping_add(1);
        }
        self.read_hives();
        let mut selected = self.selected.clone();
        while !self.key_exists(&selected) {
            match parent_of(&selected) {
                Some(parent) => selected = parent,
                None => break,
            }
        }
        self.selected = selected;
        self.expanded = core::mem::take(&mut self.expanded)
            .into_iter()
            .filter(|path| self.key_exists(path))
            .collect();
        self.fill_keys(tree);
        self.fill_values(tree);
        self.refresh(tree);
    }

    fn read_hives(&mut self) {
        let Some(registry) = self.registry.as_mut() else {
            self.user_damaged = false;
            self.machine_damaged = false;
            return;
        };
        self.user_damaged = registry.hive(HiveFile::User).is_err();
        self.machine_damaged = registry.hive(HiveFile::Machine).is_err();
    }

    fn key_exists(&mut self, path: &KeyPath) -> bool {
        self.registry
            .as_mut()
            .is_some_and(|registry| registry.key_exists(path).unwrap_or(false))
    }

    fn readable(&self, root: Root) -> bool {
        match root {
            Root::CurrentUser | Root::Users => !self.user_damaged,
            Root::LocalMachine | Root::CurrentConfig => !self.machine_damaged,
            Root::ClassesRoot => !self.user_damaged && !self.machine_damaged,
        }
    }

    fn subkeys(&mut self, path: &KeyPath) -> Vec<Vec<u16>> {
        if !self.readable(path.root) {
            return Vec::new();
        }
        self.registry
            .as_mut()
            .and_then(|registry| registry.subkeys(path).ok())
            .unwrap_or_default()
    }

    fn is_expanded(&self, path: &KeyPath) -> bool {
        self.expanded.iter().any(|open| same_key(open, path))
    }

    fn push_rows(&mut self, rows: &mut Vec<KeyRow>, path: KeyPath, depth: usize) {
        if rows.len() >= MAX_KEY_ROWS {
            return;
        }
        let children = self.subkeys(&path);
        let expanded = !children.is_empty() && self.is_expanded(&path);
        rows.push(KeyRow {
            path: path.clone(),
            depth,
            expandable: !children.is_empty(),
            expanded,
            more: 0,
            up: false,
        });
        if !expanded {
            return;
        }
        for child in children.iter().take(MAX_SUBKEYS_SHOWN) {
            let Ok(child_path) = path.join(child) else {
                continue;
            };
            self.push_rows(rows, child_path, depth + 1);
        }
        if children.len() > MAX_SUBKEYS_SHOWN && rows.len() < MAX_KEY_ROWS {
            rows.push(KeyRow {
                path,
                depth: depth + 1,
                expandable: false,
                expanded: false,
                more: children.len() - MAX_SUBKEYS_SHOWN,
                up: false,
            });
        }
    }

    fn row_text(row: &KeyRow) -> String {
        if row.up {
            return String::from(UP_ROW);
        }
        let mut text = String::new();
        match row.depth {
            depth if depth <= DEEPEST_INDENTED => {
                for _ in 0..depth {
                    text.push_str(INDENT);
                }
            }
            _ => {
                for _ in 1..DEEPEST_INDENTED {
                    text.push_str(INDENT);
                }
                text.push_str(DEEPER);
            }
        }
        if row.more > 0 {
            text.push_str(&format!(
                "and {} more: windows registry get <key> lists them all.",
                row.more
            ));
            return text;
        }
        text.push_str(match (row.expandable, row.expanded) {
            (true, true) => EXPANDED,
            (true, false) => COLLAPSED,
            (false, _) => LEAF,
        });
        text.push_str(&row.path.leaf());
        text
    }

    fn shown_name(row: &KeyRow) -> String {
        match (row.up, row.more) {
            (true, _) => String::from(UP_ROW),
            (false, 0) => row.path.leaf(),
            (false, more) => format!("and {more} more: windows registry get <key> lists them all."),
        }
    }

    fn shape_text(row: &KeyRow) -> String {
        let state = match (row.expandable, row.expanded) {
            (true, true) => SHAPE_EXPANDED,
            (true, false) => SHAPE_EXPANDABLE,
            (false, _) => SHAPE_LEAF,
        };
        format!("{}{}", row.depth.min(usize::from(u8::MAX)), char::from(state))
    }

    fn page_rows(&mut self, rows: &mut Vec<KeyRow>) {
        let Some(page) = self.page.clone() else {
            for root in Root::SHOWN {
                rows.push(KeyRow {
                    path: KeyPath::root(root),
                    depth: 0,
                    expandable: false,
                    expanded: false,
                    more: 0,
                    up: false,
                });
            }
            return;
        };
        rows.push(KeyRow {
            path: page.clone(),
            depth: 0,
            expandable: false,
            expanded: false,
            more: 0,
            up: true,
        });
        let children = self.subkeys(&page);
        for child in children.iter().take(MAX_SUBKEYS_SHOWN) {
            if let Ok(path) = page.join(child) {
                rows.push(KeyRow {
                    path,
                    depth: 0,
                    expandable: false,
                    expanded: false,
                    more: 0,
                    up: false,
                });
            }
        }
        if children.len() > MAX_SUBKEYS_SHOWN {
            rows.push(KeyRow {
                path: page,
                depth: 0,
                expandable: false,
                expanded: false,
                more: children.len() - MAX_SUBKEYS_SHOWN,
                up: false,
            });
        }
    }

    fn fill_keys(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let mut rows = Vec::new();
        if self.registry.is_some() {
            match self.drill_down {
                true => self.page_rows(&mut rows),
                false => {
                    for root in Root::SHOWN {
                        self.push_rows(&mut rows, KeyPath::root(root), 0);
                    }
                }
            }
        }
        if !rows.is_empty() {
            if self.key_slot.is_none() {
                self.key_slot = KEY_CELLS.take();
            }
            if let Some(slot) = &self.key_slot {
                for (index, row) in rows.iter().enumerate() {
                    slot.set(index, KEY_NAME, &Self::shown_name(row));
                    slot.set(index, KEY_SHAPE, &Self::shape_text(row));
                }
            }
            tree.set_cell_data(self.widgets.keys, data_of(self.key_slot.as_ref()));
        }
        self.rows = rows;
        tree.set_rows(self.widgets.keys, self.rows.len());
        if let Some(index) = self.selected_row() {
            tree.set_selected(self.widgets.keys, index);
        }
    }

    fn selected_row(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.more == 0 && !row.up && same_key(&row.path, &self.selected))
    }

    /// Browses one level a page, as a phone does (docs/ux/windows-registry-v0.3.md section 7,
    /// D401), or the whole outline, with the pane `width` wide.
    pub fn set_drill_down(&mut self, tree: &mut WidgetTree<CAPACITY>, on: bool, width: i32) {
        self.drill_down = on;
        self.page = match on {
            true => Some(self.selected.clone()),
            false => None,
        };
        let empty = match on {
            true => NO_SUBKEYS,
            false => UNREADABLE,
        };
        tree.set_empty_text(self.widgets.keys, empty);
        tree.set_drill_down(self.widgets.keys, on);
        self.lay_out(tree, width);
        self.fill_keys(tree);
        self.fill_values(tree);
        self.refresh(tree);
    }

    fn lay_out(&self, tree: &mut WidgetTree<CAPACITY>, width: i32) {
        let widgets = self.widgets;
        if !self.drill_down || width >= PHONE_WIDTH {
            let lists_height = HEIGHT - LISTS_ROOM;
            let buttons_top = HEIGHT - BUTTONS_FROM_BOTTOM;
            let places = [
                (widgets.path, at(PATH_FIELD.0, PATH_FIELD.1, PATH_FIELD.2, PATH_FIELD.3)),
                (widgets.copy, at(COPY_PATH.0, COPY_PATH.1, BUTTON.0, BUTTON.1)),
                (widgets.refresh, at(REFRESH.0, REFRESH.1, BUTTON.0, BUTTON.1)),
                (widgets.keys, at(KEYS_X_WIDTH.0, LISTS_TOP, KEYS_X_WIDTH.1, lists_height)),
                (widgets.values, at(VALUES_X_WIDTH.0, LISTS_TOP, VALUES_X_WIDTH.1, lists_height)),
                (widgets.new_key, at(BUTTON_XS[0], buttons_top, BUTTON.0, BUTTON.1)),
                (widgets.new_value, at(BUTTON_XS[1], buttons_top, BUTTON.0, BUTTON.1)),
                (widgets.edit, at(BUTTON_XS[2], buttons_top, BUTTON.0, BUTTON.1)),
                (widgets.delete, at(BUTTON_XS[3], buttons_top, BUTTON.0, BUTTON.1)),
                (widgets.replace, at(BUTTON_XS[0], buttons_top, REPLACE_WIDTH, BUTTON.1)),
                (widgets.status, status_place(PADDING, HEIGHT - STATUS_FROM_BOTTOM, LINE_WIDTH)),
            ];
            for (id, place) in places {
                tree.set_place(id, place);
            }
            return;
        }
        let full = width - 2 * PADDING;
        let half = (width - 40) / 2;
        let right = PADDING + half + PHONE_GAP;
        let values_top = PHONE_KEYS_TOP + PHONE_KEYS_HEIGHT + PHONE_GAP;
        let second = PHONE_BUTTONS_TOP + PHONE_ROW + PHONE_GAP;
        let places = [
            (widgets.path, at(PADDING, PATH_FIELD.1, full, PATH_FIELD.3)),
            (widgets.copy, at(PADDING, PHONE_COPY_TOP, half, PHONE_ROW)),
            (widgets.refresh, at(right, PHONE_COPY_TOP, half, PHONE_ROW)),
            (widgets.keys, at(PADDING, PHONE_KEYS_TOP, full, PHONE_KEYS_HEIGHT)),
            (widgets.values, at(PADDING, values_top, full, PHONE_VALUES_HEIGHT)),
            (widgets.new_key, at(PADDING, PHONE_BUTTONS_TOP, half, PHONE_ROW)),
            (widgets.new_value, at(right, PHONE_BUTTONS_TOP, half, PHONE_ROW)),
            (widgets.edit, at(PADDING, second, half, PHONE_ROW)),
            (widgets.delete, at(right, second, half, PHONE_ROW)),
            (widgets.replace, at(PADDING, PHONE_BUTTONS_TOP, full, PHONE_ROW)),
            (widgets.status, status_place(PADDING, second + PHONE_ROW + PADDING, full)),
        ];
        for (id, place) in places {
            tree.set_place(id, place);
        }
    }

    fn drill(&mut self, tree: &mut WidgetTree<CAPACITY>, row: usize) {
        let Some(shown) = self.rows.get(row).cloned() else {
            return;
        };
        if shown.more > 0 {
            return;
        }
        let target = match shown.up {
            true => parent_of(&shown.path),
            false => Some(shown.path),
        };
        self.page = target.clone();
        if let Some(target) = target {
            self.selected = target;
        }
        self.said = None;
        self.values_focused = false;
        self.fill_keys(tree);
        self.fill_values(tree);
        tree.set_selected(self.widgets.keys, 0);
        tree.set_selected(self.widgets.values, 0);
        self.refresh(tree);
    }

    fn sorted(&self, values: Vec<ValueRow>) -> Vec<ValueRow> {
        let (column, order) = self.sort;
        let mut keyed: Vec<(String, ValueRow)> = values
            .into_iter()
            .map(|row| {
                let key = match column {
                    0 => String::new(),
                    1 => type_name(row.value.kind),
                    _ => shown_data(row.value.kind, &row.value.data),
                };
                (key, row)
            })
            .collect();
        keyed.sort_by(|(one_key, one), (other_key, other)| {
            match (one.value.name.is_empty(), other.value.name.is_empty()) {
                (true, false) => return Order::Less,
                (false, true) => return Order::Greater,
                _ => {}
            }
            let ordered = match column {
                0 => compare_names(&one.value.name, &other.value.name),
                _ => one_key.cmp(other_key),
            };
            match order {
                SortOrder::Ascending => ordered,
                SortOrder::Descending => ordered.reverse(),
            }
        });
        keyed.into_iter().map(|(_, row)| row).collect()
    }

    fn fill_values(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let readable = self.readable(self.selected.root);
        let read = match (readable, self.registry.as_mut()) {
            (true, Some(registry)) => registry.values(&self.selected).ok(),
            _ => None,
        };
        let mut values: Vec<ValueRow> = read
            .unwrap_or_default()
            .into_iter()
            .map(|value| ValueRow { value, set: true })
            .collect();
        if readable && !values.iter().any(|row| row.value.name.is_empty()) {
            values.push(ValueRow {
                value: Value {
                    name: Vec::new(),
                    kind: REG_SZ,
                    data: Vec::new(),
                },
                set: false,
            });
        }
        let mut values = self.sorted(values);
        values.truncate(MAX_VALUE_ROWS);
        if !values.is_empty() {
            if self.value_slot.is_none() {
                self.value_slot = VALUE_CELLS.take();
            }
            if let Some(slot) = &self.value_slot {
                for (index, row) in values.iter().enumerate() {
                    slot.set(index, 0, &name_text(&row.value.name));
                    slot.set(index, 1, &type_name(row.value.kind));
                    slot.set(index, 2, &cell_data(row));
                }
            }
            tree.set_cell_data(self.widgets.values, data_of(self.value_slot.as_ref()));
        }
        self.values = values;
        tree.set_rows(self.widgets.values, self.values.len());
    }

    fn select_value_named(&mut self, tree: &mut WidgetTree<CAPACITY>, name: &[u16]) {
        if let Some(index) = self
            .values
            .iter()
            .position(|row| same_name(&row.value.name, name))
        {
            tree.set_selected(self.widgets.values, index);
            self.values_focused = true;
        }
    }

    fn selected_value(&self, tree: &WidgetTree<CAPACITY>) -> Option<&ValueRow> {
        self.values.get(tree.selected(self.widgets.values)?)
    }

    fn may_change(&mut self, path: &KeyPath) -> bool {
        let machine = matches!(path.root, Root::LocalMachine | Root::CurrentConfig);
        let admin = self
            .registry
            .as_mut()
            .is_some_and(|registry| registry.may_change_machine());
        self.registry.is_some()
            && self.readable(path.root)
            && (!machine || admin)
            && path.root != Root::Users
    }

    fn status_line(&mut self) -> (String, Token) {
        if self.user_damaged {
            return (String::from(USER_DAMAGED_LINE), Token::Text);
        }
        if let Some(said) = &self.said {
            return said.clone();
        }
        let machine = matches!(self.selected.root, Root::LocalMachine | Root::CurrentConfig);
        if machine && self.machine_damaged {
            return (String::from(MACHINE_DAMAGED_LINE), Token::Text);
        }
        if self.selected.names.is_empty() {
            return (String::from(ROOT_LINE), Token::TextDim);
        }
        let admin = self
            .registry
            .as_mut()
            .is_some_and(|registry| registry.may_change_machine());
        let line = match self.selected.root {
            Root::LocalMachine | Root::CurrentConfig if admin => EVERY_USER_LINE,
            Root::LocalMachine | Root::CurrentConfig => MACHINE_WIDE_LINE,
            Root::ClassesRoot => CLASSES_LINE,
            _ => {
                let software = KeyPath {
                    root: Root::CurrentUser,
                    names: alloc::vec![utf16("Software")],
                };
                match self.subkeys(&software).is_empty() {
                    true => NOTHING_YET_LINE,
                    false => NORMAL_LINE,
                }
            }
        };
        (String::from(line), Token::TextDim)
    }

    fn refresh(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let path_text = self.selected.text();
        if tree.focus() != Some(widgets.path) {
            tree.set_text(widgets.path, &field_text(&path_text));
        }
        let (words, ink) = self.status_line();
        show_line(tree, widgets.status, &words, ink);
        let free = self.panel == Panel::Closed;
        let selected = self.selected.clone();
        let writable = self.may_change(&selected);
        let root = selected.names.is_empty();
        let value_set = self.selected_value(tree).is_some_and(|row| row.set);
        let deletes_value = self.values_focused && value_set;
        for id in [
            widgets.new_key,
            widgets.new_value,
            widgets.edit,
            widgets.delete,
        ] {
            tree.set_shown(id, !self.user_damaged);
        }
        tree.set_shown(widgets.replace, self.user_damaged);
        tree.set_enabled(widgets.new_key, free && writable);
        tree.set_enabled(widgets.new_value, free && writable);
        tree.set_enabled(widgets.edit, free && writable && !root);
        tree.set_enabled(widgets.delete, free && writable && (deletes_value || !root));
        tree.set_enabled(widgets.replace, free);
        for id in [
            widgets.path,
            widgets.copy,
            widgets.refresh,
            widgets.keys,
            widgets.values,
        ] {
            tree.set_enabled(id, free);
        }
    }

    fn select(&mut self, tree: &mut WidgetTree<CAPACITY>, path: KeyPath) {
        let mut above = parent_of(&path);
        while let Some(parent) = above {
            if !self.is_expanded(&parent) {
                self.expanded.push(parent.clone());
            }
            above = parent_of(&parent);
        }
        self.selected = path;
        self.said = None;
        self.values_focused = false;
        self.fill_keys(tree);
        self.fill_values(tree);
        tree.set_selected(self.widgets.values, 0);
        self.refresh(tree);
    }

    fn toggle(&mut self, tree: &mut WidgetTree<CAPACITY>, row: usize) {
        let Some(shown) = self.rows.get(row).cloned() else {
            return;
        };
        if !shown.expandable || shown.more > 0 {
            return;
        }
        match shown.expanded {
            true => {
                self.expanded.retain(|open| !same_key(open, &shown.path));
                self.expanded.retain(|open| !is_under(open, &shown.path));
                if is_under(&self.selected, &shown.path) {
                    self.selected = shown.path.clone();
                    self.fill_values(tree);
                }
            }
            false => self.expanded.push(shown.path),
        }
        self.fill_keys(tree);
        self.refresh(tree);
    }

    fn go_to_typed_path(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let typed = String::from(tree.text_of(self.widgets.path).unwrap_or("").trim());
        let path = KeyPath::parse(&typed).ok();
        match path {
            Some(path) if self.key_exists(&path) => {
                self.select(tree, path);
                let shown = field_text(&self.selected.text());
                tree.set_text(self.widgets.path, &shown);
            }
            _ => {
                self.said = Some((command::message(Error::NoSuchKey, &typed, ""), Token::Text));
                self.refresh(tree);
            }
        }
    }

    fn panel_widget(&self) -> Option<WidgetId> {
        let widgets = self.widgets;
        match self.panel {
            Panel::Closed => None,
            Panel::Edit { .. } => Some(widgets.edit_panel),
            Panel::NewKey | Panel::NewValue => Some(widgets.name_panel),
            Panel::DeleteKey(_) | Panel::DeleteValue(_) | Panel::Replace => {
                Some(widgets.confirm_panel)
            }
        }
    }

    fn place_panel(&self, tree: &mut WidgetTree<CAPACITY>, panel: WidgetId, height: i32) {
        tree.set_place(
            panel,
            at(PANEL_X, (HEIGHT - height) / 2, PANEL_WIDTH, height),
        );
    }

    fn show_panel(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        panel: Panel,
        primary: WidgetId,
        focus: WidgetId,
        now_ms: u32,
    ) {
        if let Some(open) = self.panel_widget() {
            tree.set_shown(open, false);
        }
        self.panel = panel;
        if let Some(shown) = self.panel_widget() {
            tree.set_shown(shown, true);
        }
        tree.set_default_button(primary);
        self.refresh(tree);
        tree.set_focus(Some(focus), now_ms);
    }

    fn close_panel(&mut self, tree: &mut WidgetTree<CAPACITY>, focus_at_ms: Option<u32>) {
        if let Some(open) = self.panel_widget() {
            tree.set_shown(open, false);
        }
        self.panel = Panel::Closed;
        tree.set_default_button(self.default_button);
        self.refresh(tree);
        if let Some(now_ms) = focus_at_ms {
            let focus = match self.opener {
                Opener::Keys => self.widgets.keys,
                Opener::Values => self.widgets.values,
                Opener::Button(id) => id,
            };
            tree.set_focus(Some(focus), now_ms);
        }
    }

    fn open_edit(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        value: Value,
        created: bool,
        now_ms: u32,
    ) {
        let widgets = self.widgets;
        let kind = value.kind;
        let number = matches!(kind, REG_DWORD | REG_QWORD);
        let shown_name = value_shown(&value);
        let title = match value.name.is_empty() {
            true => String::from("Edit the (Default) value"),
            false => format!("Edit {shown_name}"),
        };
        show_line(tree, widgets.edit_title, &title, Token::Text);
        let sub = format!("{} in {}", type_name(kind), self.selected.text());
        show_line(tree, widgets.edit_sub, &sub, Token::TextDim);
        let (text, refusal) = match kind {
            REG_DWORD | REG_QWORD => (
                format!("{:x}", number_of(kind, &value.data).unwrap_or(0)),
                None,
            ),
            REG_SZ | REG_EXPAND_SZ => (string_of(&value.data), None),
            REG_BINARY => (hex_text(&value.data, " "), None),
            REG_MULTI_SZ => (
                data_text(kind, &value.data),
                Some(format!(
                    "A {} value cannot be edited here yet: windows registry set replaces it.",
                    type_name(kind)
                )),
            ),
            _ => (
                data_text(kind, &value.data),
                Some(format!(
                    "A {} value cannot be edited here.",
                    type_name(kind)
                )),
            ),
        };
        let refusal = match (refusal, text.len() > FIELD_BYTES) {
            (Some(refusal), _) => Some(refusal),
            (None, true) => Some(format!(
                "Too large to edit here: {}. windows registry set replaces it.",
                eggos_components::messages::size_text(value.data.len() as u64)
            )),
            (None, false) => None,
        };
        let editable = refusal.is_none();
        let height = match number {
            true => NUMBER_PANEL,
            false => TEXT_PANEL,
        };
        self.place_panel(tree, widgets.edit_panel, height);
        let line_top = match number {
            true => PANEL_LOW_LINE,
            false => PANEL_SECOND_ROW,
        };
        tree.set_place(
            widgets.edit_line,
            Place::at(
                PADDING,
                line_top,
                Length::Px(PANEL_TEXT_WIDTH),
                Length::Content,
            ),
        );
        let buttons_top = height - PANEL_BUTTONS_FROM_BOTTOM;
        tree.set_place(
            widgets.edit_cancel,
            at(PANEL_CANCEL_X, buttons_top, BUTTON.0, BUTTON.1),
        );
        tree.set_place(
            widgets.edit_save,
            at(PANEL_PRIMARY_X, buttons_top, BUTTON.0, BUTTON.1),
        );
        tree.set_text(widgets.edit_field, &field_text(&text));
        tree.set_enabled(widgets.edit_field, editable);
        tree.set_shown(widgets.edit_bases, number);
        tree.set_selected(widgets.edit_bases, 0);
        tree.set_enabled(widgets.edit_save, editable);
        match &refusal {
            Some(refusal) => show_line(tree, widgets.edit_line, refusal, Token::TextDim),
            None => show_line(tree, widgets.edit_line, "", Token::Text),
        }
        let focus = match editable {
            true => widgets.edit_field,
            false => widgets.edit_cancel,
        };
        self.show_panel(
            tree,
            Panel::Edit {
                value,
                created,
                hexadecimal: true,
                editable,
            },
            widgets.edit_save,
            focus,
            now_ms,
        );
    }

    fn typed_data(&self, tree: &WidgetTree<CAPACITY>) -> Result<Vec<u8>, String> {
        let Panel::Edit {
            value, hexadecimal, ..
        } = &self.panel
        else {
            return Err(String::new());
        };
        let text = tree.text_of(self.widgets.edit_field).unwrap_or("");
        match value.kind {
            REG_DWORD | REG_QWORD => {
                let number = parse_typed_number(text, *hexadecimal)
                    .filter(|&number| number <= number_limit(value.kind))
                    .ok_or_else(|| not_a_number(value.kind))?;
                Ok(match value.kind {
                    REG_DWORD => (number as u32).to_le_bytes().to_vec(),
                    _ => number.to_le_bytes().to_vec(),
                })
            }
            REG_BINARY => parse_typed_binary(text).map_err(String::from),
            _ => Ok(string_data(text)),
        }
    }

    fn check_edit(&self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let Panel::Edit { editable: true, .. } = self.panel else {
            return;
        };
        match self.typed_data(tree) {
            Ok(_) => {
                show_line(tree, widgets.edit_line, "", Token::Text);
                tree.set_enabled(widgets.edit_save, true);
            }
            Err(validation) => {
                show_line(tree, widgets.edit_line, &validation, Token::Text);
                tree.set_enabled(widgets.edit_save, false);
            }
        }
    }

    fn switch_base(&mut self, tree: &mut WidgetTree<CAPACITY>, hexadecimal_now: bool) {
        let field = self.widgets.edit_field;
        let Panel::Edit {
            hexadecimal, value, ..
        } = &mut self.panel
        else {
            return;
        };
        if *hexadecimal == hexadecimal_now || !matches!(value.kind, REG_DWORD | REG_QWORD) {
            return;
        }
        let typed = parse_typed_number(tree.text_of(field).unwrap_or(""), *hexadecimal);
        *hexadecimal = hexadecimal_now;
        if let Some(number) = typed {
            let text = match hexadecimal_now {
                true => format!("{number:x}"),
                false => format!("{number}"),
            };
            tree.set_text(field, &text);
        }
        self.check_edit(tree);
    }

    fn save(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let Panel::Edit {
            value,
            created,
            editable: true,
            ..
        } = self.panel.clone()
        else {
            return;
        };
        let data = match self.typed_data(tree) {
            Ok(data) => data,
            Err(_) => return,
        };
        let path = self.selected.clone();
        let written = match self.registry.as_mut() {
            Some(registry) => registry.set_value(&path, &value.name, value.kind, data),
            None => Err(Error::WriteFailed),
        };
        match written {
            Ok(()) => {
                let shown = value_shown(&value);
                let said = match created {
                    true => format!("Created {shown}."),
                    false => format!("Saved {shown}."),
                };
                self.close_panel(tree, Some(now_ms));
                self.fill_values(tree);
                self.select_value_named(tree, &value.name);
                self.said = Some((said, Token::Text));
                self.refresh(tree);
            }
            Err(error) => {
                let message = command::message(error, &path.text(), &name_text(&value.name));
                show_line(tree, self.widgets.edit_line, &message, Token::Text);
            }
        }
    }

    fn cancel_edit(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let created = match &self.panel {
            Panel::Edit { value, created, .. } => created.then(|| value_shown(value)),
            _ => None,
        };
        self.close_panel(tree, Some(now_ms));
        if let Some(shown) = created {
            self.said = Some((format!("Created {shown} with no data."), Token::Text));
            self.refresh(tree);
        }
    }

    fn open_name(&mut self, tree: &mut WidgetTree<CAPACITY>, for_key: bool, now_ms: u32) {
        let widgets = self.widgets;
        let height = match for_key {
            true => KEY_PANEL,
            false => VALUE_PANEL,
        };
        self.place_panel(tree, widgets.name_panel, height);
        let what = match for_key {
            true => "key",
            false => "value",
        };
        let title = format!("New {what} in {}", self.selected.leaf());
        show_line(tree, widgets.name_title, &title, Token::Text);
        set_placeholder(
            tree,
            widgets.name_field,
            match for_key {
                true => KEY_PLACEHOLDER,
                false => VALUE_PLACEHOLDER,
            },
        );
        tree.set_text(widgets.name_field, "");
        tree.set_shown(widgets.name_type, !for_key);
        tree.set_selected(widgets.name_type, 0);
        let line_top = match for_key {
            true => PANEL_SECOND_ROW,
            false => PANEL_LOW_LINE,
        };
        tree.set_place(
            widgets.name_line,
            Place::at(
                PADDING,
                line_top,
                Length::Px(PANEL_TEXT_WIDTH),
                Length::Content,
            ),
        );
        let buttons_top = height - PANEL_BUTTONS_FROM_BOTTOM;
        tree.set_place(
            widgets.name_cancel,
            at(PANEL_CANCEL_X, buttons_top, BUTTON.0, BUTTON.1),
        );
        tree.set_place(
            widgets.name_create,
            at(PANEL_PRIMARY_X, buttons_top, BUTTON.0, BUTTON.1),
        );
        let panel = match for_key {
            true => Panel::NewKey,
            false => Panel::NewValue,
        };
        self.show_panel(tree, panel, widgets.name_create, widgets.name_field, now_ms);
        self.check_name(tree);
    }

    fn name_refusal(&mut self, tree: &WidgetTree<CAPACITY>) -> Option<String> {
        let typed = String::from(tree.text_of(self.widgets.name_field).unwrap_or(""));
        let name = utf16(&typed);
        match self.panel {
            Panel::NewKey => {
                if self.selected.names.len() >= MAX_DEPTH {
                    return Some(format!(
                        "This key is as deep as the registry goes ({MAX_DEPTH} levels)."
                    ));
                }
                if typed.contains('\\') {
                    return Some(String::from("Key names cannot contain \\ ."));
                }
                if name.len() > MAX_KEY_NAME_UNITS {
                    return Some(format!(
                        "Key names are {MAX_KEY_NAME_UNITS} characters at most."
                    ));
                }
                let selected = self.selected.clone();
                if !name.is_empty()
                    && self
                        .subkeys(&selected)
                        .iter()
                        .any(|existing| same_name(existing, &name))
                {
                    return Some(format!("A key named {typed} already exists here."));
                }
                None
            }
            Panel::NewValue => {
                if name.len() > MAX_VALUE_NAME_UNITS {
                    return Some(format!(
                        "Value names are {} characters at most.",
                        grouped(MAX_VALUE_NAME_UNITS)
                    ));
                }
                if self
                    .values
                    .iter()
                    .any(|row| row.set && same_name(&row.value.name, &name))
                {
                    return Some(format!(
                        "A value named {} already exists here.",
                        name_text(&name)
                    ));
                }
                None
            }
            _ => None,
        }
    }

    fn check_name(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let widgets = self.widgets;
        let typed_empty = tree
            .text_of(widgets.name_field)
            .is_none_or(|text| text.is_empty());
        let mut sub = format!("Under {}", self.selected.text());
        if self.panel == Panel::NewValue && typed_empty {
            sub.push_str(" \u{b7} an empty name is the (Default) value");
        }
        show_line(tree, widgets.name_sub, &sub, Token::TextDim);
        let refusal = self.name_refusal(tree);
        let blocked = refusal.is_some() || (self.panel == Panel::NewKey && typed_empty);
        show_line(
            tree,
            widgets.name_line,
            refusal.as_deref().unwrap_or(""),
            Token::Text,
        );
        tree.set_enabled(widgets.name_create, !blocked);
    }

    fn create(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let widgets = self.widgets;
        if !tree.is_enabled(widgets.name_create) || self.name_refusal(tree).is_some() {
            return;
        }
        let typed = String::from(tree.text_of(widgets.name_field).unwrap_or(""));
        let name = utf16(&typed);
        let selected = self.selected.clone();
        match self.panel {
            Panel::NewKey => {
                let made = selected
                    .join(&name)
                    .and_then(|path| match self.registry.as_mut() {
                        Some(registry) => registry.create_key(&path).map(|_| path),
                        None => Err(Error::WriteFailed),
                    });
                match made {
                    Ok(path) => {
                        self.close_panel(tree, Some(now_ms));
                        self.select(tree, path);
                        self.said = Some((format!("Created {typed}."), Token::Text));
                        self.refresh(tree);
                    }
                    Err(error) => {
                        let message = command::message(error, &selected.text(), "");
                        show_line(tree, widgets.name_line, &message, Token::Text);
                    }
                }
            }
            Panel::NewValue => {
                let kind = tree
                    .selected(widgets.name_type)
                    .and_then(|index| EDITABLE.get(index).copied())
                    .unwrap_or(REG_SZ);
                let data = empty_data(kind);
                let written = match self.registry.as_mut() {
                    Some(registry) => registry.set_value(&selected, &name, kind, data.clone()),
                    None => Err(Error::WriteFailed),
                };
                match written {
                    Ok(()) => {
                        self.close_panel(tree, None);
                        self.fill_values(tree);
                        self.select_value_named(tree, &name);
                        self.opener = Opener::Values;
                        self.open_edit(tree, Value { name, kind, data }, true, now_ms);
                    }
                    Err(error) => {
                        let message = command::message(error, &selected.text(), &name_text(&name));
                        show_line(tree, widgets.name_line, &message, Token::Text);
                    }
                }
            }
            _ => {}
        }
    }

    fn open_delete(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let widgets = self.widgets;
        let selected = self.selected.clone();
        if !self.may_change(&selected) {
            return;
        }
        let value = match self.values_focused {
            true => self.selected_value(tree).filter(|row| row.set).cloned(),
            false => None,
        };
        let (panel, title, detail, note) = match value {
            Some(row) => {
                let title = match row.value.name.is_empty() {
                    true => String::from("Delete the (Default) value?"),
                    false => format!("Delete the value {}?", utf8(&row.value.name)),
                };
                let detail = match row.value.name.is_empty() {
                    true => String::from("It becomes (value not set)."),
                    false => format!(
                        "{} \u{b7} {}",
                        type_name(row.value.kind),
                        data_text(row.value.kind, &row.value.data)
                    ),
                };
                (
                    Panel::DeleteValue(row.value.name),
                    title,
                    detail,
                    String::from("This cannot be undone."),
                )
            }
            None if !selected.names.is_empty() => {
                let counts = self.registry.as_mut().and_then(|registry| {
                    let below = registry.count_below(&selected).ok()?;
                    let own = registry.values(&selected).ok()?.len();
                    Some((below.0, below.1 + own))
                });
                let (subkeys, values) = counts.unwrap_or((0, 0));
                let note = match selected.root == Root::ClassesRoot && self.machine_has(&selected) {
                    true => "Removes your copy; the machine's stays.",
                    false => "This cannot be undone.",
                };
                (
                    Panel::DeleteKey(selected.clone()),
                    format!("Delete {}?", selected.leaf()),
                    key_contents(subkeys, values),
                    String::from(note),
                )
            }
            None => return,
        };
        show_line(tree, widgets.confirm_title, &title, Token::Text);
        show_line(tree, widgets.confirm_detail, &detail, Token::TextDim);
        show_line(tree, widgets.confirm_note, &note, Token::TextDim);
        tree.set_text(widgets.confirm_primary, "Delete");
        self.show_panel(
            tree,
            panel,
            widgets.confirm_primary,
            widgets.confirm_cancel,
            now_ms,
        );
    }

    fn machine_has(&mut self, path: &KeyPath) -> bool {
        let machine = KeyPath {
            root: Root::LocalMachine,
            names: CLASSES
                .iter()
                .map(|name| utf16(name))
                .chain(path.names.iter().cloned())
                .collect(),
        };
        self.key_exists(&machine)
    }

    fn open_replace(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let widgets = self.widgets;
        let file = USER_HIVE_PATH.rsplit('/').next().unwrap_or("user.hive");
        show_line(
            tree,
            widgets.confirm_title,
            "Replace the damaged registry file?",
            Token::Text,
        );
        show_line(
            tree,
            widgets.confirm_detail,
            &format!("The damaged file is kept beside it as {file}{DAMAGED_SUFFIX}."),
            Token::TextDim,
        );
        show_line(
            tree,
            widgets.confirm_note,
            "Windows programs' saved settings are lost unless that file is repaired by hand.",
            Token::TextDim,
        );
        tree.set_text(widgets.confirm_primary, "Replace");
        self.show_panel(
            tree,
            Panel::Replace,
            widgets.confirm_primary,
            widgets.confirm_cancel,
            now_ms,
        );
    }

    fn confirmed(&mut self, tree: &mut WidgetTree<CAPACITY>, now_ms: u32) {
        let panel = self.panel.clone();
        let Some(registry) = self.registry.as_mut() else {
            return;
        };
        let selected = self.selected.clone();
        let (done, said) = match &panel {
            Panel::DeleteKey(path) => (
                registry.delete_key(path, true),
                format!("Deleted {}.", path.leaf()),
            ),
            Panel::DeleteValue(name) => (
                registry.delete_value(&selected, name),
                format!("Deleted {}.", name_text(name)),
            ),
            Panel::Replace => (registry.reset(HiveFile::User), String::from(REPLACED_LINE)),
            _ => return,
        };
        match done {
            Ok(()) => {
                let value_row = tree.selected(self.widgets.values).unwrap_or(0);
                self.close_panel(tree, Some(now_ms));
                match panel {
                    Panel::DeleteKey(path) => {
                        let parent = parent_of(&path).unwrap_or(path);
                        self.select(tree, parent);
                    }
                    Panel::DeleteValue(_) => {
                        self.fill_values(tree);
                        tree.set_selected(self.widgets.values, value_row.saturating_sub(1));
                    }
                    _ => {
                        self.selected = KeyPath::root(Root::CurrentUser);
                        self.reread(tree);
                    }
                }
                self.said = Some((said, Token::Text));
                self.refresh(tree);
            }
            Err(error) => {
                let (path, name) = match &panel {
                    Panel::DeleteKey(path) => (path.text(), String::new()),
                    Panel::DeleteValue(name) => (selected.text(), name_text(name)),
                    _ => (String::new(), String::new()),
                };
                let message = command::message(error, &path, &name);
                show_line(tree, self.widgets.confirm_note, &message, Token::Text);
            }
        }
    }

    fn shown(&self, tree: &WidgetTree<CAPACITY>) -> bool {
        tree.is_shown(self.widgets.keys)
    }

    /// Acts on what an input did at `now_ms` when it was for the pane.
    pub fn respond(
        &mut self,
        tree: &mut WidgetTree<CAPACITY>,
        response: Response,
        now_ms: u32,
    ) -> Reply {
        let widgets = self.widgets;
        let focus = tree.focus();
        match response {
            Response::RowToggled(id, row, _) if id == widgets.keys && self.drill_down => {
                self.drill(tree, row);
            }
            Response::Chosen(id, _) if id == widgets.keys && self.drill_down => {}
            Response::Chosen(id, row) if id == widgets.keys => {
                match self.rows.get(row).cloned() {
                    Some(shown) if shown.more == 0 => self.select(tree, shown.path),
                    _ => {
                        if let Some(index) = self.selected_row() {
                            tree.set_selected(widgets.keys, index);
                        }
                    }
                }
                self.values_focused = false;
            }
            Response::RowToggled(id, row, _) if id == widgets.keys => self.toggle(tree, row),
            Response::Chosen(id, _) if id == widgets.values => {
                self.values_focused = true;
                self.said = None;
                self.refresh(tree);
            }
            Response::Opened(id, row) if id == widgets.values => {
                self.values_focused = true;
                if let Some(value) = self.values.get(row).map(|row| row.value.clone()) {
                    let selected = self.selected.clone();
                    if self.may_change(&selected) && !selected.names.is_empty() {
                        self.opener = Opener::Values;
                        self.open_edit(tree, value, false, now_ms);
                    }
                }
            }
            Response::Sorted(id, column, order) if id == widgets.values => {
                self.sort = (column, order);
                self.fill_values(tree);
            }
            Response::Activated(id) if id == widgets.copy => {
                self.to_copy = self.selected.text();
                self.said = Some((format!("Copied {}.", self.to_copy), Token::Text));
                self.refresh(tree);
                return Reply::Done(true);
            }
            Response::Activated(id) if id == widgets.refresh => {
                self.said = None;
                self.reread(tree);
            }
            Response::Activated(id) if id == widgets.new_key || id == widgets.new_value => {
                self.opener = Opener::Button(id);
                self.open_name(tree, id == widgets.new_key, now_ms);
            }
            Response::Activated(id) if id == widgets.edit => {
                let value = self.selected_value(tree).map(|row| row.value.clone());
                if let Some(value) = value {
                    self.opener = Opener::Button(id);
                    self.open_edit(tree, value, false, now_ms);
                }
            }
            Response::Activated(id) if id == widgets.delete => {
                self.opener = Opener::Button(id);
                self.open_delete(tree, now_ms);
            }
            Response::Activated(id) if id == widgets.replace => {
                self.opener = Opener::Button(id);
                self.open_replace(tree, now_ms);
            }
            Response::Activated(id) if id == widgets.edit_save => self.save(tree, now_ms),
            Response::Activated(id) if id == widgets.edit_cancel => self.cancel_edit(tree, now_ms),
            Response::Activated(id) if id == widgets.name_create => self.create(tree, now_ms),
            Response::Activated(id)
                if id == widgets.name_cancel || id == widgets.confirm_cancel =>
            {
                self.close_panel(tree, Some(now_ms));
            }
            Response::Activated(id) if id == widgets.confirm_primary => {
                self.confirmed(tree, now_ms);
            }
            Response::Edited(id) if id == widgets.edit_field => self.check_edit(tree),
            Response::Edited(id) if id == widgets.name_field => self.check_name(tree),
            Response::Edited(id) if id == widgets.path => {}
            Response::Chosen(id, base) if id == widgets.edit_bases => {
                self.switch_base(tree, base == 0);
            }
            Response::Chosen(id, _) if id == widgets.name_type => {}
            Response::Unused { code: key::ESC, .. } if self.panel != Panel::Closed => {
                match self.panel {
                    Panel::Edit { .. } => self.cancel_edit(tree, now_ms),
                    _ => self.close_panel(tree, Some(now_ms)),
                }
            }
            Response::Unused {
                code: key::ENTER, ..
            } if focus == Some(widgets.path) => self.go_to_typed_path(tree),
            Response::Unused {
                code: key::BACKSPACE,
                ..
            } if focus == Some(widgets.keys) => {
                if let Some(parent) = parent_of(&self.selected) {
                    self.select(tree, parent);
                }
            }
            Response::Unused {
                code: key::DELETE, ..
            } if focus == Some(widgets.keys) || focus == Some(widgets.values) => {
                self.values_focused = focus == Some(widgets.values);
                self.opener = match self.values_focused {
                    true => Opener::Values,
                    false => Opener::Keys,
                };
                self.open_delete(tree, now_ms);
            }
            Response::Unused { code: F5, .. }
                if self.shown(tree) && self.panel == Panel::Closed =>
            {
                self.said = None;
                self.reread(tree);
            }
            _ => return Reply::NotMine,
        }
        Reply::Done(false)
    }

    /// The key outline's rows as text: indented by level, `+` before a closed key with
    /// subkeys and `-` before an open one, as the outline's glyphs show them.
    pub fn key_rows(&self) -> Vec<String> {
        self.rows.iter().map(Self::row_text).collect()
    }

    /// The value table's rows: name, type and data cells.
    pub fn value_rows(&self) -> Vec<[String; 3]> {
        self.values
            .iter()
            .map(|row| {
                [
                    name_text(&row.value.name),
                    type_name(row.value.kind),
                    cell_data(row),
                ]
            })
            .collect()
    }

    /// The status line.
    pub fn status<'a>(&self, tree: &'a WidgetTree<CAPACITY>) -> Option<&'a str> {
        tree.text_of(self.widgets.status)
    }

    /// Whether a panel shows.
    pub fn panel_open(&self) -> bool {
        self.panel != Panel::Closed
    }
}

#[cfg(test)]
mod tests;
