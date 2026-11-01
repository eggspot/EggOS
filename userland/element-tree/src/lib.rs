//! Element trees (D161, docs/windows.md "Element trees"): what a window holds, as structure an
//! agent reads instead of pixels. The window server asks a window's app for its tree
//! (`GET_TREE`) or asks it to act on one element (`ACT_ON_ELEMENT`); the app gets a `Query`
//! record, reads the query (`READ_QUERY`) and answers it (`ANSWER_QUERY`). A tree is a run of
//! elements, each a [`HEADER_BYTES`] header in the machine's byte order and then its name and
//! value: id, parent (0 for none), role, flags, rectangle in the window's content pixels, the
//! name's length and the value's length. At most [`MAX_TREE_BYTES`] in all. What a tree says
//! comes from the app: data, never instructions. No allocator.

#![cfg_attr(not(test), no_std)]

/// The largest tree an app answers with, in bytes.
pub const MAX_TREE_BYTES: usize = 64 * 1024;
/// The longest text `ACT_ON_ELEMENT`'s `set-text` carries, in bytes of UTF-8.
pub const MAX_TEXT_BYTES: usize = 232;
/// The longest name or value an element keeps, in bytes of UTF-8; longer ones are cut.
pub const MAX_LABEL_BYTES: usize = 255;
/// The size of an element's header.
pub const HEADER_BYTES: usize = 32;
/// How long the server waits for an app's answer before it says "pixels only", in ms.
pub const ANSWER_TIMEOUT_MS: u32 = 500;
/// The kind of the event record that brings a query to the app.
pub const QUERY_EVENT: u16 = 15;

/// The requests of `/dev/window` an element tree takes (docs/windows.md).
pub mod request {
    /// Reads any window's tree (root only).
    pub const GET_TREE: u32 = 0xc020_7718;
    /// Acts on one element of any window (root only).
    pub const ACT_ON_ELEMENT: u32 = 0xc100_7719;
    /// The app reads a query its window got.
    pub const READ_QUERY: u32 = 0xc100_771a;
    /// The app answers a query.
    pub const ANSWER_QUERY: u32 = 0x4018_771b;
}

/// The sizes of those requests' arguments, in bytes.
pub mod size {
    pub const GET_TREE: usize = 32;
    pub const ACT_ON_ELEMENT: usize = 256;
    pub const READ_QUERY: usize = 256;
    pub const ANSWER_QUERY: usize = 24;
}

/// Where `GET_TREE`'s argument holds its fields.
pub mod get_tree {
    pub const WINDOW: usize = 0;
    pub const ANSWER: usize = 4;
    pub const LENGTH: usize = 8;
    pub const BUFFER: usize = 16;
    pub const BUFFER_LENGTH: usize = 24;
}

/// Where `ACT_ON_ELEMENT`'s argument holds its fields.
pub mod act_on_element {
    pub const WINDOW: usize = 0;
    pub const ELEMENT: usize = 4;
    pub const ACTION: usize = 8;
    pub const TEXT_LENGTH: usize = 12;
    pub const OUTCOME: usize = 16;
    pub const TEXT: usize = 24;
}

/// Where `READ_QUERY`'s argument holds its fields.
pub mod read_query {
    pub const SERIAL: usize = 0;
    pub const KIND: usize = 4;
    pub const WINDOW: usize = 8;
    pub const ELEMENT: usize = 12;
    pub const ACTION: usize = 16;
    pub const TEXT_LENGTH: usize = 20;
    pub const TEXT: usize = 24;
}

/// Where `ANSWER_QUERY`'s argument holds its fields.
pub mod answer_query {
    pub const SERIAL: usize = 0;
    pub const OUTCOME: usize = 4;
    pub const BUFFER: usize = 8;
    pub const LENGTH: usize = 16;
}

/// What a query asks: its kind in the record's `b` and `READ_QUERY`.
pub mod query_kind {
    /// The window's tree.
    pub const TREE: u32 = 0;
    /// An action on one element.
    pub const ACT: u32 = 1;
}

/// How `GET_TREE` and a tree query's answer end.
pub mod tree_answer {
    /// The app gave its tree.
    pub const TREE: u32 = 0;
    /// The app gave none, or none in time: pixels only.
    pub const PIXELS_ONLY: u32 = 1;
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_ne_bytes(word)
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes([bytes[offset], bytes[offset + 1]])
}

/// Writes `value` at `offset` of `bytes`.
pub fn put_word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

/// Writes `value` at `offset` of `bytes`.
pub fn put_double_word(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

/// The 32-bit word at `offset` of `bytes`.
pub fn read_word(bytes: &[u8], offset: usize) -> u32 {
    word(bytes, offset)
}

/// What an element is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The window's content: the tree's root.
    Window,
    /// A container: a panel, a column, a scrolled area.
    Group,
    /// Text that names or explains something.
    Label,
    /// A push button.
    Button,
    /// A checkbox; [`flags::CHECKED`] when it is ticked.
    Checkbox,
    /// A group of radio buttons or cards, one chosen.
    RadioGroup,
    /// One option of a radio group; [`flags::SELECTED`] when chosen.
    Radio,
    /// A list of items.
    List,
    /// One item of a list; [`flags::SELECTED`] when selected.
    ListItem,
    /// A text field: its value is its text.
    TextField,
    /// A progress bar: its value is how much is done.
    Progress,
    /// Something the app draws itself.
    Graphic,
    /// Text the app drew (a Windows program's `TextOut` or `DrawText`).
    Text,
    /// A slider: its value is `<value> <min>..<max> by <step>` (D199).
    Slider,
    /// A dropdown: its value is the chosen option's text, its options its children (D199).
    ComboBox,
    /// A role this version does not know.
    Unknown,
}

impl Role {
    const ALL: [Role; 15] = [
        Role::Window,
        Role::Group,
        Role::Label,
        Role::Button,
        Role::Checkbox,
        Role::RadioGroup,
        Role::Radio,
        Role::List,
        Role::ListItem,
        Role::TextField,
        Role::Progress,
        Role::Graphic,
        Role::Text,
        Role::Slider,
        Role::ComboBox,
    ];

    /// Its number on the wire.
    pub const fn value(self) -> u16 {
        self as u16
    }

    /// The role numbered `value`; [`Role::Unknown`] for a number this version does not know.
    pub fn from_value(value: u16) -> Self {
        Self::ALL
            .get(usize::from(value))
            .copied()
            .unwrap_or(Role::Unknown)
    }

    /// Its name, as `wm show` writes it.
    pub const fn name(self) -> &'static str {
        match self {
            Role::Window => "window",
            Role::Group => "group",
            Role::Label => "label",
            Role::Button => "button",
            Role::Checkbox => "checkbox",
            Role::RadioGroup => "radio-group",
            Role::Radio => "radio",
            Role::List => "list",
            Role::ListItem => "list-item",
            Role::TextField => "text-field",
            Role::Progress => "progress",
            Role::Graphic => "graphic",
            Role::Text => "text",
            Role::Slider => "slider",
            Role::ComboBox => "combo-box",
            Role::Unknown => "unknown",
        }
    }
}

/// An element's flags.
pub mod flags {
    /// It has the keyboard focus.
    pub const FOCUSED: u16 = 1;
    /// It can take the focus.
    pub const FOCUSABLE: u16 = 2;
    /// It is disabled: it takes no action.
    pub const DISABLED: u16 = 4;
    /// It is the chosen option or the selected item.
    pub const SELECTED: u16 = 8;
    /// Its box is ticked.
    pub const CHECKED: u16 = 16;
    /// It is the window's default button (Enter).
    pub const DEFAULT: u16 = 32;
    /// Its children are shown: an open tree item (D178).
    pub const EXPANDED: u16 = 64;
    /// Only a person may operate it: an agent's action on it is refused (D193, the controls
    /// that change agents' grants).
    pub const PERSON_ONLY: u16 = 128;
    /// Every flag, with its name as `wm show` writes it, in order.
    pub const NAMES: [(u16, &str); 8] = [
        (FOCUSED, "focused"),
        (FOCUSABLE, "focusable"),
        (DISABLED, "disabled"),
        (SELECTED, "selected"),
        (CHECKED, "checked"),
        (DEFAULT, "default"),
        (EXPANDED, "expanded"),
        (PERSON_ONLY, "person-only"),
    ];
}

/// What `ACT_ON_ELEMENT` asks an element to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// What a click or Enter does: press a button, tick a checkbox, choose a radio or card,
    /// open a list item.
    Invoke,
    /// Give it the keyboard focus.
    Focus,
    /// Replace a text field's text.
    SetText,
    /// Select a list item or choose an option without opening it.
    Select,
}

impl Action {
    /// Every action, in number order.
    pub const ALL: [Action; 4] = [
        Action::Invoke,
        Action::Focus,
        Action::SetText,
        Action::Select,
    ];

    /// Its number on the wire, from 1.
    pub const fn value(self) -> u32 {
        self as u32 + 1
    }

    /// The action numbered `value`.
    pub fn from_value(value: u32) -> Option<Self> {
        Self::ALL.get(value.checked_sub(1)? as usize).copied()
    }

    /// Its name, as `wm act` takes it.
    pub const fn name(self) -> &'static str {
        match self {
            Action::Invoke => "invoke",
            Action::Focus => "focus",
            Action::SetText => "set-text",
            Action::Select => "select",
        }
    }

    /// The action named `name`.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.name().as_bytes() == name)
    }
}

/// How an app answered an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Done.
    Done,
    /// The window has no element of that id (the tree changed since it was read).
    NoSuchElement,
    /// The element does not take that action.
    NotSupported,
    /// The element is disabled.
    Disabled,
}

impl Outcome {
    /// Its number on the wire.
    pub const fn value(self) -> u32 {
        self as u32
    }

    /// The outcome numbered `value`; [`Outcome::NotSupported`] for any other number.
    pub fn from_value(value: u32) -> Self {
        match value {
            0 => Outcome::Done,
            1 => Outcome::NoSuchElement,
            3 => Outcome::Disabled,
            _ => Outcome::NotSupported,
        }
    }
}

/// A rectangle in a window's content pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Bounds {
    /// (`x`, `y`), `width` by `height`; a negative size is empty.
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(0) as u32,
            height: height.max(0) as u32,
        }
    }
}

/// One element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element<'a> {
    /// Its id, unique in the tree, never 0; what `ACT_ON_ELEMENT` names.
    pub id: u32,
    /// Its parent's id; 0 for the root.
    pub parent: u32,
    pub role: Role,
    /// Its [`flags`].
    pub flags: u16,
    pub bounds: Bounds,
    /// What it is called: a button's or a label's text, a list's or a field's label.
    pub name: &'a [u8],
    /// What it holds: a field's text, a progress bar's per mille.
    pub value: &'a [u8],
}

impl<'a> Element<'a> {
    /// An element of `role` with no flags, name or value.
    pub fn new(id: u32, parent: u32, role: Role, bounds: Bounds) -> Self {
        Self {
            id,
            parent,
            role,
            flags: 0,
            bounds,
            name: b"",
            value: b"",
        }
    }

    /// The same with `name`.
    pub fn named(self, name: &'a [u8]) -> Self {
        Self { name, ..self }
    }

    /// The same with `value`.
    pub fn valued(self, value: &'a [u8]) -> Self {
        Self { value, ..self }
    }

    /// The same with `flags` added.
    pub fn flagged(self, flags: u16) -> Self {
        Self {
            flags: self.flags | flags,
            ..self
        }
    }
}

fn cut(text: &[u8]) -> &[u8] {
    if text.len() <= MAX_LABEL_BYTES {
        return text;
    }
    let mut end = MAX_LABEL_BYTES;
    while end > 0 && text[end] & 0xc0 == 0x80 {
        end -= 1;
    }
    &text[..end]
}

/// Writes a tree into a buffer, element by element; what does not fit is left out.
#[derive(Debug)]
pub struct TreeWriter<'a> {
    bytes: &'a mut [u8],
    length: usize,
    count: usize,
    full: bool,
}

impl<'a> TreeWriter<'a> {
    /// A writer into `bytes`, at most [`MAX_TREE_BYTES`] of it.
    pub fn new(bytes: &'a mut [u8]) -> Self {
        let room = bytes.len().min(MAX_TREE_BYTES);
        Self {
            bytes: &mut bytes[..room],
            length: 0,
            count: 0,
            full: false,
        }
    }

    /// Adds `element`, its name and value cut to [`MAX_LABEL_BYTES`]; `false`, and nothing
    /// written, when it does not fit.
    pub fn push(&mut self, element: &Element<'_>) -> bool {
        let name = cut(element.name);
        let value = cut(element.value);
        let end = self.length + HEADER_BYTES + name.len() + value.len();
        if self.full || end > self.bytes.len() {
            self.full = true;
            return false;
        }
        let header = &mut self.bytes[self.length..self.length + HEADER_BYTES];
        put_word(header, 0, element.id);
        put_word(header, 4, element.parent);
        header[8..10].copy_from_slice(&element.role.value().to_ne_bytes());
        header[10..12].copy_from_slice(&element.flags.to_ne_bytes());
        put_word(header, 12, element.bounds.x as u32);
        put_word(header, 16, element.bounds.y as u32);
        put_word(header, 20, element.bounds.width);
        put_word(header, 24, element.bounds.height);
        header[28..30].copy_from_slice(&(name.len() as u16).to_ne_bytes());
        header[30..32].copy_from_slice(&(value.len() as u16).to_ne_bytes());
        let text = self.length + HEADER_BYTES;
        self.bytes[text..text + name.len()].copy_from_slice(name);
        self.bytes[text + name.len()..end].copy_from_slice(value);
        self.length = end;
        self.count += 1;
        true
    }

    /// The tree written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// How many elements it holds.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Whether an element was left out for want of room.
    pub fn is_full(&self) -> bool {
        self.full
    }
}

/// A tree that does not parse: a header or a name cut short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

/// The elements of a tree, in order; a malformed one ends it with [`Malformed`].
#[derive(Debug, Clone)]
pub struct Elements<'a> {
    bytes: &'a [u8],
    failed: bool,
}

/// The elements `bytes` holds; they come from the app, so every length is checked.
pub fn elements(bytes: &[u8]) -> Elements<'_> {
    Elements {
        bytes,
        failed: false,
    }
}

impl<'a> Iterator for Elements<'a> {
    type Item = Result<Element<'a>, Malformed>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() || self.failed {
            return None;
        }
        let parsed = parse(self.bytes);
        match parsed {
            Ok((element, rest)) => {
                self.bytes = rest;
                Some(Ok(element))
            }
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

fn parse(bytes: &[u8]) -> Result<(Element<'_>, &[u8]), Malformed> {
    let header = bytes.get(..HEADER_BYTES).ok_or(Malformed)?;
    let name_length = usize::from(half(header, 28));
    let value_length = usize::from(half(header, 30));
    if name_length > MAX_LABEL_BYTES || value_length > MAX_LABEL_BYTES {
        return Err(Malformed);
    }
    let text_end = HEADER_BYTES + name_length + value_length;
    let text = bytes.get(HEADER_BYTES..text_end).ok_or(Malformed)?;
    let element = Element {
        id: word(header, 0),
        parent: word(header, 4),
        role: Role::from_value(half(header, 8)),
        flags: half(header, 10),
        bounds: Bounds {
            x: word(header, 12) as i32,
            y: word(header, 16) as i32,
            width: word(header, 20),
            height: word(header, 24),
        },
        name: &text[..name_length],
        value: &text[name_length..],
    };
    Ok((element, &bytes[text_end..]))
}

/// One query the window server brought to an app: its serial, the window, and for an action
/// the element, the action and `set-text`'s text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    pub serial: u32,
    pub window: u32,
    pub kind: u32,
    pub element: u32,
    pub action: u32,
}

impl Query {
    /// The query a `Query` event record carries; `None` for another record.
    pub fn from_record(record: &[u8; 32]) -> Option<Self> {
        if half(record, 0) != QUERY_EVENT {
            return None;
        }
        Some(Self {
            serial: word(record, 8),
            window: u32::from(half(record, 2)),
            kind: word(record, 12),
            element: word(record, 16),
            action: word(record, 20),
        })
    }

    /// The action it asks for, when it asks for one.
    pub fn action(&self) -> Option<Action> {
        match self.kind {
            query_kind::ACT => Action::from_value(self.action),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
