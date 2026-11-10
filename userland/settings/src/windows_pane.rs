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
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use eggos_app::files::{Files, Kind as FileKind};
use eggos_runtime::errno::Errno;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Align, Column, Escape, Kind, Length, Place, SortOrder, Table};
use eggos_windows_registry::command::{self, VALUE_NOT_SET, name_text};
use eggos_windows_registry::hive::Value;
use eggos_windows_registry::path::{KeyPath, Root, utf8, utf16};
use eggos_windows_registry::registry::{
    DAMAGED_SUFFIX, HiveFile, MACHINE_HIVE_PATH, Registry, Store, StoreError, TEMPORARY_SUFFIX,
    USER_HIVE_PATH,
};
use eggos_windows_registry::value::{
    EDITABLE, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ, data_text,
    hex_text, number_of, parse_hex, string_data, string_of, strings_data, type_name,
};
use eggos_windows_registry::{
    Error, MAX_DEPTH, MAX_HIVE_BYTES, MAX_KEY_NAME_UNITS, MAX_VALUE_NAME_UNITS, compare_names,
    same_name,
};

/// The pane's height: the outline and the table fill what the rows above and below leave.
pub const HEIGHT: i32 = 348;
/// The widgets the pane adds to its container.
pub const WIDGETS: usize = 12 + 8 + 8 + 6;
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
const CELL_BYTES: usize = 63;
const SLOTS: usize = 2;
const KEY_COLUMNS: [Column; 1] = [Column {
    title: "Keys",
    width: 168,
    min_width: 64,
    align: Align::Start,
    sortable: false,
}];
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

struct Cell {
    length: AtomicU8,
    bytes: [AtomicU8; CELL_BYTES],
}

impl Cell {
    const fn new() -> Self {
        Self {
            length: AtomicU8::new(0),
            bytes: [const { AtomicU8::new(0) }; CELL_BYTES],
        }
    }

    fn store(&self, text: &str) {
        let mut length = text.len().min(CELL_BYTES);
        while !text.is_char_boundary(length) {
            length -= 1;
        }
        for (cell, &byte) in self.bytes.iter().zip(&text.as_bytes()[..length]) {
            cell.store(byte, Ordering::Relaxed);
        }
        self.length.store(length as u8, Ordering::Release);
    }

    fn write(&self, out: &mut Text<64>) {
        let length = usize::from(self.length.load(Ordering::Acquire));
        let mut bytes = [0u8; CELL_BYTES];
        for (byte, cell) in bytes.iter_mut().zip(&self.bytes[..length]) {
            *byte = cell.load(Ordering::Relaxed);
        }
        out.push_str(core::str::from_utf8(&bytes[..length]).unwrap_or("?"));
    }
}

struct KeySlot {
    taken: AtomicBool,
    cells: [Cell; MAX_KEY_ROWS],
}

struct ValueSlot {
    taken: AtomicBool,
    cells: [[Cell; 3]; MAX_VALUE_ROWS],
}

static KEY_SLOTS: [KeySlot; SLOTS] = [const {
    KeySlot {
        taken: AtomicBool::new(false),
        cells: [const { Cell::new() }; MAX_KEY_ROWS],
    }
}; SLOTS];
static VALUE_SLOTS: [ValueSlot; SLOTS] = [const {
    ValueSlot {
        taken: AtomicBool::new(false),
        cells: [const { [const { Cell::new() }; 3] }; MAX_VALUE_ROWS],
    }
}; SLOTS];
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

fn take_slot(taken: impl Fn(usize) -> &'static AtomicBool) -> usize {
    loop {
        let start = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
        for step in 0..SLOTS {
            let slot = (start + step) % SLOTS;
            if taken(slot)
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return slot;
            }
        }
        core::hint::spin_loop();
    }
}

fn key_cell(data: u32, row: usize, _: usize, out: &mut Text<64>) {
    if let Some(cell) = KEY_SLOTS
        .get(data as usize)
        .and_then(|slot| slot.cells.get(row))
    {
        cell.write(out);
    }
}

fn value_cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    if let Some(cell) = VALUE_SLOTS
        .get(data as usize)
        .and_then(|slot| slot.cells.get(row))
        .and_then(|cells| cells.get(column))
    {
        cell.write(out);
    }
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

    fn user_id(&mut self) -> u32 {
        self.files.user_id()
    }

    fn now(&mut self) -> u64 {
        self.files.now()
    }
}

/// The hive files on a file system: the user's below `home`, the machine's at
/// [`MACHINE_HIVE_PATH`], each written through its temporary twin and a rename, as the Windows
/// layer writes them.
pub struct FileStore<F: Files> {
    files: F,
    home: String,
    user_id: u32,
    clock: fn() -> u64,
}

impl<F: Files> core::fmt::Debug for FileStore<F> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("FileStore")
            .field("home", &self.home)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

impl<F: Files> FileStore<F> {
    /// The hives of user `user_id`, whose home folder is `home`, through `files`; `clock`
    /// gives the time in 100 ns since 1601.
    pub fn new(files: F, home: &str, user_id: u32, clock: fn() -> u64) -> Self {
        Self {
            files,
            home: String::from(home.trim_end_matches('/')),
            user_id,
            clock,
        }
    }

    fn path(&self, file: HiveFile, suffix: &str) -> String {
        match file {
            HiveFile::User => format!("{}/{USER_HIVE_PATH}{suffix}", self.home),
            HiveFile::Machine => format!("{MACHINE_HIVE_PATH}{suffix}"),
        }
    }
}

fn store_error(errno: Errno) -> StoreError {
    match errno {
        Errno::ENOSPC => StoreError::DiskFull,
        _ => StoreError::Failed,
    }
}

impl<F: Files> Store for FileStore<F> {
    fn read(&mut self, file: HiveFile) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.path(file, "");
        let status = match self.files.status_following(path.as_bytes()) {
            Ok(status) => status,
            Err(Errno::ENOENT) => return Ok(None),
            Err(_) => return Err(StoreError::Failed),
        };
        if status.kind != FileKind::File {
            return Err(StoreError::Failed);
        }
        let wanted = (status.size as usize).min(MAX_HIVE_BYTES + 1);
        let handle = self
            .files
            .open(path.as_bytes())
            .map_err(|_| StoreError::Failed)?;
        let mut bytes = alloc::vec![0u8; wanted];
        let mut length = 0;
        let read = loop {
            if length == bytes.len() {
                break Ok(());
            }
            match self.files.read(handle, &mut bytes[length..]) {
                Ok(0) => break Ok(()),
                Ok(count) => length += count,
                Err(_) => break Err(StoreError::Failed),
            }
        };
        self.files.close(handle);
        read?;
        bytes.truncate(length);
        Ok(Some(bytes))
    }

    fn stamp(&mut self, file: HiveFile) -> Option<u64> {
        let path = self.path(file, "");
        let status = self.files.status_following(path.as_bytes()).ok()?;
        Some(
            status
                .modified
                .wrapping_mul(1_000_000_007)
                .wrapping_add(status.size),
        )
    }

    fn write_temporary(&mut self, file: HiveFile, bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.path(file, TEMPORARY_SUFFIX);
        for (at, _) in path.match_indices('/').skip(1) {
            let _ = self.files.make_directory(&path.as_bytes()[..at]);
        }
        let _ = self.files.remove_file(path.as_bytes());
        let handle = self
            .files
            .create(path.as_bytes(), 0o600)
            .map_err(store_error)?;
        let written = self
            .files
            .write(handle, bytes)
            .and_then(|()| self.files.sync_file(handle));
        self.files.close(handle);
        written.map_err(store_error)
    }

    fn replace_with_temporary(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let from = self.path(file, TEMPORARY_SUFFIX);
        let to = self.path(file, "");
        self.files
            .rename(from.as_bytes(), to.as_bytes())
            .map_err(store_error)
    }

    fn set_aside(&mut self, file: HiveFile) -> Result<(), StoreError> {
        let from = self.path(file, "");
        let to = self.path(file, DAMAGED_SUFFIX);
        match self.files.rename(from.as_bytes(), to.as_bytes()) {
            Ok(()) | Err(Errno::ENOENT) => Ok(()),
            Err(errno) => Err(store_error(errno)),
        }
    }

    fn user_id(&mut self) -> u32 {
        self.user_id
    }

    fn now(&mut self) -> u64 {
        (self.clock)()
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
    values: Vec<ValueRow>,
    sort: (usize, SortOrder),
    values_focused: bool,
    panel: Panel,
    opener: Opener,
    said: Option<(String, Token)>,
    to_copy: String,
    user_damaged: bool,
    machine_damaged: bool,
    key_slot: Option<usize>,
    value_slot: Option<usize>,
    default_button: WidgetId,
}

impl Drop for WindowsPane {
    fn drop(&mut self) {
        if let Some(slot) = self.key_slot {
            KEY_SLOTS[slot].taken.store(false, Ordering::Release);
        }
        if let Some(slot) = self.value_slot {
            VALUE_SLOTS[slot].taken.store(false, Ordering::Release);
        }
    }
}

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn at(x: i32, y: i32, width: i32, height: i32) -> Place {
    Place::at(x, y, Length::Px(width), Length::Px(height))
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
        true => data_text(row.value.kind, &row.value.data),
        false => String::from(VALUE_NOT_SET),
    }
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
        let mut keys_table = Table::new("Keys", &KEY_COLUMNS, 0, key_cell, 0);
        keys_table.empty_text = UNREADABLE;
        let keys = add(
            tree,
            pane,
            at(KEYS_X_WIDTH.0, LISTS_TOP, KEYS_X_WIDTH.1, lists_height),
            Kind::Table(keys_table),
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
            add(
                tree,
                pane,
                at(PANEL_X, (HEIGHT - height) / 2, PANEL_WIDTH, height),
                Kind::Custom {
                    painter: format_panel,
                    data: 0,
                },
            )
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
            });
        }
    }

    fn row_text(row: &KeyRow) -> String {
        let mut text = String::new();
        for _ in 0..row.depth {
            text.push_str(INDENT);
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

    fn fill_keys(&mut self, tree: &mut WidgetTree<CAPACITY>) {
        let mut rows = Vec::new();
        if self.registry.is_some() {
            for root in Root::SHOWN {
                self.push_rows(&mut rows, KeyPath::root(root), 0);
            }
        }
        if !rows.is_empty() {
            let slot = *self
                .key_slot
                .get_or_insert_with(|| take_slot(|slot| &KEY_SLOTS[slot].taken));
            for (index, row) in rows.iter().enumerate() {
                KEY_SLOTS[slot].cells[index].store(&Self::row_text(row));
            }
            tree.set_cell_data(self.widgets.keys, slot as u32);
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
            .position(|row| row.more == 0 && same_key(&row.path, &self.selected))
    }

    fn sorted(&self, mut values: Vec<ValueRow>) -> Vec<ValueRow> {
        let (column, order) = self.sort;
        let text = |row: &ValueRow| match column {
            1 => type_name(row.value.kind),
            _ => data_text(row.value.kind, &row.value.data),
        };
        values.sort_by(|one, other| {
            match (one.value.name.is_empty(), other.value.name.is_empty()) {
                (true, false) => return Order::Less,
                (false, true) => return Order::Greater,
                _ => {}
            }
            let ordered = match column {
                0 => compare_names(&one.value.name, &other.value.name),
                _ => text(one).cmp(&text(other)),
            };
            match order {
                SortOrder::Ascending => ordered,
                SortOrder::Descending => ordered.reverse(),
            }
        });
        values
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
            let slot = *self
                .value_slot
                .get_or_insert_with(|| take_slot(|slot| &VALUE_SLOTS[slot].taken));
            for (index, row) in values.iter().enumerate() {
                let cells = &VALUE_SLOTS[slot].cells[index];
                cells[0].store(&name_text(&row.value.name));
                cells[1].store(&type_name(row.value.kind));
                cells[2].store(&cell_data(row));
            }
            tree.set_cell_data(self.widgets.values, slot as u32);
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
            Response::Opened(id, row) if id == widgets.keys => self.toggle(tree, row),
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

    /// The key list's rows as drawn.
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
