//! What the screen reader says for an element (D603, docs/ux/accessibility-v0.4.md section
//! 1.4): one line, `name, role, value, state, position`, each part left out when empty. The
//! window server, `screen-reader read` and the `screen_reader_read` tool all word elements
//! here, so the three say the same. Also the utterance's record, as the server's log and
//! `READ_READER_LOG` carry it. No allocator.

use crate::{Element, Elements, Role, flags};
use core::fmt::{self, Write};

/// The longest utterance, in bytes of UTF-8; a longer one is cut at a character.
pub const MAX_UTTERANCE_BYTES: usize = 255;
/// The size of an utterance record's header in `READ_READER_LOG`'s answer.
pub const UTTERANCE_HEADER_BYTES: usize = 16;

/// Where an utterance came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    On,
    Focus,
    Window,
    Container,
    Change,
    Echo,
    Shell,
    Notice,
    Keys,
    Agent,
    Read,
    Say,
}

impl Source {
    const ALL: [Self; 12] = [
        Self::On,
        Self::Focus,
        Self::Window,
        Self::Container,
        Self::Change,
        Self::Echo,
        Self::Shell,
        Self::Notice,
        Self::Keys,
        Self::Agent,
        Self::Read,
        Self::Say,
    ];

    /// Its word in the log.
    pub const fn name(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Focus => "focus",
            Self::Window => "window",
            Self::Container => "container",
            Self::Change => "change",
            Self::Echo => "echo",
            Self::Shell => "shell",
            Self::Notice => "notice",
            Self::Keys => "keys",
            Self::Agent => "agent",
            Self::Read => "read",
            Self::Say => "say",
        }
    }

    /// The source numbered `value` on the wire.
    pub fn from_value(value: u8) -> Self {
        Self::ALL
            .get(usize::from(value))
            .copied()
            .unwrap_or(Self::Say)
    }
}

/// Whether an utterance replaces what shows, dropping the queue, or waits its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Interrupt,
    Queue,
}

impl Priority {
    /// Its word in the log.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Queue => "queue",
        }
    }
}

/// One utterance as `READ_READER_LOG` writes it: a header (serial, the clock in ms, source,
/// priority, text length), then the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtteranceRecord<'a> {
    pub serial: u32,
    pub at_ms: u64,
    pub source: Source,
    pub priority: Priority,
    pub text: &'a str,
}

impl<'a> UtteranceRecord<'a> {
    /// Writes it at the start of `out`; the bytes written, or `None` without room.
    pub fn write(&self, out: &mut [u8]) -> Option<usize> {
        let text = self.text.as_bytes();
        let total = UTTERANCE_HEADER_BYTES + text.len();
        let out = out.get_mut(..total)?;
        out[0..4].copy_from_slice(&self.serial.to_ne_bytes());
        out[4..12].copy_from_slice(&self.at_ms.to_ne_bytes());
        out[12] = self.source as u8;
        out[13] = self.priority as u8;
        out[14] = text.len() as u8;
        out[15] = 0;
        out[UTTERANCE_HEADER_BYTES..].copy_from_slice(text);
        Some(total)
    }

    /// The record at the start of `bytes` and what follows it; `None` when it is cut or its
    /// text is not UTF-8.
    pub fn read(bytes: &'a [u8]) -> Option<(Self, &'a [u8])> {
        let header = bytes.get(..UTTERANCE_HEADER_BYTES)?;
        let length = usize::from(header[14]);
        let text = bytes.get(UTTERANCE_HEADER_BYTES..UTTERANCE_HEADER_BYTES + length)?;
        let record = Self {
            serial: u32::from_ne_bytes(header[0..4].try_into().ok()?),
            at_ms: u64::from_ne_bytes(header[4..12].try_into().ok()?),
            source: Source::from_value(header[12]),
            priority: match header[13] {
                0 => Priority::Interrupt,
                _ => Priority::Queue,
            },
            text: core::str::from_utf8(text).ok()?,
        };
        Some((record, &bytes[UTTERANCE_HEADER_BYTES + length..]))
    }
}

/// An element's place among its siblings that share its role: `index` of `count`, from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub index: u16,
    pub count: u16,
}

fn role_words(element: &Element<'_>, parent_role: Option<Role>) -> &'static str {
    match element.role {
        Role::Window => "window",
        Role::Group => "group",
        Role::Label | Role::Text | Role::Tooltip => "",
        Role::Button => "button",
        Role::Checkbox => "checkbox",
        Role::RadioGroup => "radio group",
        Role::Radio => "radio button",
        Role::List => "list",
        Role::ListItem => "list item",
        Role::TextField => "text field",
        Role::TextArea => "text area",
        Role::Slider => "slider",
        Role::ComboBox => "combo box",
        Role::Progress => "progress",
        Role::MenuBar => "menu bar",
        Role::MenuItem if parent_role == Some(Role::MenuBar) => "menu",
        Role::MenuItem => "menu item",
        Role::Table => "table",
        Role::Row => "row",
        Role::ColumnHeader => "column header",
        Role::Tree => "outline",
        Role::TreeItem => "item",
        Role::Graphic => "graphic",
        Role::Unknown => "control",
    }
}

/// A sink that writes parts separated by `, ` and leaves empty ones out, cutting at
/// [`MAX_UTTERANCE_BYTES`].
struct Parts<'w, W: Write> {
    out: &'w mut W,
    written: usize,
    any: bool,
}

impl<W: Write> Parts<'_, W> {
    fn part(&mut self, text: &str) -> fmt::Result {
        if text.is_empty() {
            return Ok(());
        }
        if self.any {
            self.raw(", ")?;
        }
        self.any = true;
        self.raw(text)
    }

    fn raw(&mut self, text: &str) -> fmt::Result {
        let room = MAX_UTTERANCE_BYTES.saturating_sub(self.written);
        let mut cut = text.len().min(room);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.written += cut;
        self.out.write_str(&text[..cut])
    }

    fn formatted(&mut self, arguments: fmt::Arguments<'_>) -> fmt::Result {
        let mut text = ShortText::new();
        let _ = text.write_fmt(arguments);
        self.part(text.as_str())
    }
}

struct ShortText {
    bytes: [u8; MAX_UTTERANCE_BYTES],
    length: usize,
}

impl ShortText {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_UTTERANCE_BYTES],
            length: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl Write for ShortText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = self.bytes.len() - self.length;
        let mut cut = text.len().min(room);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.bytes[self.length..self.length + cut].copy_from_slice(&text.as_bytes()[..cut]);
        self.length += cut;
        Ok(())
    }
}

fn text_of(bytes: &[u8]) -> &str {
    match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => core::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or(""),
    }
}

fn write_value(parts: &mut Parts<'_, impl Write>, element: &Element<'_>) -> fmt::Result {
    let value = text_of(element.value);
    match element.role {
        Role::Slider => {
            let mut words = value.split(' ');
            let now = words.next().unwrap_or("");
            let range = words.next().unwrap_or("");
            let (low, high) = range.split_once("..").unwrap_or(("", ""));
            let unit = words.skip_while(|word| *word != "by").nth(2).unwrap_or("");
            match (unit.is_empty(), low.is_empty()) {
                (_, true) => parts.part(now),
                (true, false) => parts.formatted(format_args!("{now}, {low} to {high}")),
                (false, false) => parts.formatted(format_args!("{now} {unit}, {low} to {high}")),
            }
        }
        Role::Progress => match value.strip_suffix('%') {
            Some(percent) => parts.formatted(format_args!("{percent} percent")),
            None => parts.part(value),
        },
        Role::TextField if value.is_empty() => parts.part("empty"),
        Role::TextArea => parts.part(value.lines().next().unwrap_or("")),
        Role::Row | Role::TreeItem => Ok(()),
        _ => parts.part(value),
    }
}

/// Writes the line for `element` into `out`: its name, role, value, state and `position`
/// among its siblings, `parent_role` being its parent's. A tree item says its `level` (its
/// path's depth) before its position. At most [`MAX_UTTERANCE_BYTES`].
pub fn utterance(
    element: &Element<'_>,
    parent_role: Option<Role>,
    position: Option<Position>,
    out: &mut impl Write,
) -> fmt::Result {
    let mut parts = Parts {
        out,
        written: 0,
        any: false,
    };
    match element.role {
        Role::Row => {
            let cells = text_of(element.value);
            let mut first = true;
            for cell in cells.split('\t') {
                match first {
                    true => parts.part(cell)?,
                    false => {
                        parts.raw(", ")?;
                        parts.raw(cell)?;
                    }
                }
                first = false;
            }
        }
        _ => parts.part(text_of(element.name))?,
    }
    parts.part(role_words(element, parent_role))?;
    write_value(&mut parts, element)?;
    let flagged = |flag: u16| element.flags & flag != 0;
    if flagged(flags::DISABLED) {
        parts.part("dimmed")?;
    }
    if element.role == Role::Checkbox {
        parts.part(match flagged(flags::CHECKED) {
            true => "checked",
            false => "not checked",
        })?;
    }
    if flagged(flags::SELECTED) {
        parts.part("selected")?;
    }
    if flagged(flags::EXPANDED) {
        parts.part("expanded")?;
    } else if flagged(flags::EXPANDABLE) {
        parts.part("collapsed")?;
    }
    if flagged(flags::DEFAULT) {
        parts.part("default")?;
    }
    if flagged(flags::NOT_RESPONDING) {
        parts.part("not responding")?;
    }
    if element.role == Role::TreeItem {
        let depth = text_of(element.value).split('/').filter(|part| !part.is_empty()).count();
        if depth > 0 {
            parts.formatted(format_args!("level {depth}"))?;
        }
    }
    if let Some(position) = position.filter(|position| position.count > 1) {
        parts.formatted(format_args!("{} of {}", position.index, position.count))?;
    }
    Ok(())
}

/// Writes a container's line: its own line without a position, `Time zones, list, 32 items`;
/// a list or radio group counts its children of the item role.
pub fn container_line(
    container: &Element<'_>,
    children: u16,
    out: &mut impl Write,
) -> fmt::Result {
    let mut parts = Parts {
        out,
        written: 0,
        any: false,
    };
    parts.part(text_of(container.name))?;
    match container.role {
        Role::List => parts.formatted(format_args!("list, {children} items")),
        Role::RadioGroup => parts.formatted(format_args!("radio group, {children} options")),
        _ => {
            parts.part(role_words(container, None))?;
            write_value(&mut parts, container)
        }
    }
}

/// Whether the reading keys stop at `element`: every element but the window and its groups.
pub fn is_read(element: &Element<'_>) -> bool {
    !matches!(element.role, Role::Window | Role::Group)
}

/// Element `index`'s place among the elements of `tree` with its parent and role, and its
/// parent's role.
pub fn place_of(tree: Elements<'_>, index: usize) -> (Option<Position>, Option<Role>) {
    let mut elements = tree.clone().flatten();
    let Some(element) = elements.nth(index) else {
        return (None, None);
    };
    let parent_role = tree
        .clone()
        .flatten()
        .find(|other| other.id == element.parent)
        .map(|parent| parent.role);
    let mut seen = 0u16;
    let mut place = 0u16;
    for (at, sibling) in tree.flatten().enumerate() {
        if sibling.parent == element.parent && sibling.role == element.role {
            seen = seen.saturating_add(1);
            if at == index {
                place = seen;
            }
        }
    }
    let position = (seen > 1).then_some(Position {
        index: place,
        count: seen,
    });
    (position, parent_role)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bounds;
    use std::string::String;

    fn line(element: Element<'_>, parent: Option<Role>, position: Option<Position>) -> String {
        let mut out = String::new();
        utterance(&element, parent, position, &mut out).unwrap();
        out
    }

    fn element(role: Role) -> Element<'static> {
        Element::new(7, 1, role, Bounds::new(0, 0, 10, 10))
    }

    #[test]
    fn a_line_is_name_role_value_state_and_position() {
        assert_eq!(
            line(element(Role::Button).named(b"Save"), None, None),
            "Save, button"
        );
        assert_eq!(
            line(
                element(Role::ListItem)
                    .named(b"Europe/Paris")
                    .flagged(flags::SELECTED),
                None,
                Some(Position { index: 3, count: 32 })
            ),
            "Europe/Paris, list item, selected, 3 of 32"
        );
        assert_eq!(
            line(element(Role::Checkbox).named(b"High contrast"), None, None),
            "High contrast, checkbox, not checked"
        );
        assert_eq!(
            line(
                element(Role::Slider)
                    .named(b"Hold a key for")
                    .valued(b"300 100..1000 by 50 ms")
                    .flagged(flags::DISABLED),
                None,
                None
            ),
            "Hold a key for, slider, 300 ms, 100 to 1000, dimmed"
        );
        assert_eq!(
            line(element(Role::TextField).named(b"City"), None, None),
            "City, text field, empty"
        );
        assert_eq!(
            line(element(Role::Progress).named(b"Installing Files").valued(b"40%"), None, None),
            "Installing Files, progress, 40 percent"
        );
        assert_eq!(
            line(element(Role::MenuItem).named(b"File"), Some(Role::MenuBar), Some(Position {
                index: 1,
                count: 4
            })),
            "File, menu, 1 of 4"
        );
    }

    #[test]
    fn a_container_counts_its_items() {
        let mut out = String::new();
        container_line(&element(Role::List).named(b"Time zones"), 32, &mut out).unwrap();
        assert_eq!(out, "Time zones, list, 32 items");
    }

    #[test]
    fn a_record_reads_back_as_written() {
        let record = UtteranceRecord {
            serial: 1203,
            at_ms: 812_345,
            source: Source::Focus,
            priority: Priority::Interrupt,
            text: "Save, button",
        };
        let mut bytes = [0u8; 64];
        let length = record.write(&mut bytes).unwrap();
        let (read, rest) = UtteranceRecord::read(&bytes[..length]).unwrap();
        assert_eq!(read, record);
        assert!(rest.is_empty());
    }
}
