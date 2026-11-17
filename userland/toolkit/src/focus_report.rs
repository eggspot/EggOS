//! The window's focused element, reported to the window server while the screen reader or zoom
//! is on (D603, docs/ux/accessibility-v0.4.md 1.2): on every focus move, on every change of the
//! focused element's value or state, and per character typed into a text field. The window
//! server asks for the reports with `READER_CHANGED`; while it has not, nothing is described
//! and nothing is sent.

use crate::application::View;
use crate::window::Window;
use eggos_element_tree::{
    Bounds, Element, HEADER_BYTES, MAX_LABEL_BYTES, MAX_TREE_BYTES, Role, TreeWriter, elements,
    flags, reader,
};

/// The record kind with which the window server asks for the reports, its word 1 or 0.
pub const READER_CHANGED: u16 = 24;

const FOCUS_MOVED: u32 = 0;
const VALUE_CHANGED: u32 = 1;
const CHARACTER_TYPED: u32 = 2;
const REPORT_BYTES: usize = 2 * (HEADER_BYTES + 2 * MAX_LABEL_BYTES);
const FINGERPRINT_SEED: u64 = 0xcbf2_9ce4_8422_2325;
const FINGERPRINT_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Whether the window server wants the reports, and what the last one said.
#[derive(Debug, Default)]
pub struct FocusReports {
    wanted: bool,
    last: Option<(u32, u64)>,
}

fn fingerprint(element: &Element<'_>) -> u64 {
    let flags = element.flags.to_ne_bytes();
    let separator = [0xffu8];
    [&flags[..], element.name, &separator, element.value]
        .into_iter()
        .flatten()
        .fold(FINGERPRINT_SEED, |hash, &byte| {
            (hash ^ u64::from(byte)).wrapping_mul(FINGERPRINT_PRIME)
        })
}

fn is_container(role: Role) -> bool {
    matches!(
        role,
        Role::Group | Role::List | Role::RadioGroup | Role::MenuBar | Role::Table | Role::Tree
    )
}

fn takes_text(role: Role) -> bool {
    matches!(role, Role::TextField | Role::TextArea)
}

/// Writes `view`'s element tree into `bytes` as `GET_TREE` answers it, the window's root first;
/// its length, or `None` when the view is pixels only.
pub fn write_tree(window: &Window, view: &mut impl View, bytes: &mut [u8]) -> Option<usize> {
    let mut tree = TreeWriter::new(bytes);
    let scale = window.scale().max(1);
    let (width, height) = window.size();
    let content = Bounds::new(0, 0, (width / scale) as i32, (height / scale) as i32);
    tree.push(&Element::new(
        crate::tree::ROOT_ELEMENT,
        0,
        Role::Window,
        content,
    ));
    view.describe(&mut tree).then(|| tree.bytes().len())
}

impl FocusReports {
    /// Takes `READER_CHANGED`'s word: on, the next report says the focused element again.
    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
        self.last = None;
    }

    /// The window got the keyboard focus: its focused element is said again.
    pub fn forget(&mut self) {
        self.last = None;
    }

    /// Reports the focused element when it moved or changed since the last report; `typed` is
    /// the character the last key typed, said alone when it went into the same text field.
    pub fn report(&mut self, window: &Window, view: &mut impl View, typed: Option<char>) {
        if !self.wanted {
            return;
        }
        let mut bytes = [0u8; MAX_TREE_BYTES];
        let Some(length) = write_tree(window, view, &mut bytes) else {
            self.last = None;
            return;
        };
        let tree = &bytes[..length];
        let Some((index, element)) = elements(tree)
            .flatten()
            .enumerate()
            .find(|(_, element)| element.flags & flags::FOCUSED != 0)
        else {
            self.last = None;
            return;
        };
        let print = fingerprint(&element);
        let same = self.last.is_some_and(|(id, _)| id == element.id);
        let (kind, character) = match typed {
            Some(typed) if same && takes_text(element.role) => (CHARACTER_TYPED, u32::from(typed)),
            _ if same && self.last.is_some_and(|(_, last)| last == print) => return,
            _ if same => (VALUE_CHANGED, 0),
            _ => (FOCUS_MOVED, 0),
        };
        self.last = Some((element.id, print));
        let container = container_of(tree, &element);
        let (position, _) = reader::place_of(elements(tree), index);
        let place = position.map_or((0, 0), |position| (position.index, position.count));
        let mut report = [0u8; REPORT_BYTES];
        let mut writer = TreeWriter::new(&mut report);
        writer.push(&element);
        if let Some(container) = &container {
            writer.push(container);
        }
        let _ = window.report_element(
            kind,
            character,
            place,
            container.map_or(0, |container| container.id),
            writer.bytes(),
        );
    }
}

fn container_of<'a>(tree: &'a [u8], element: &Element<'_>) -> Option<Element<'a>> {
    let mut parent = element.parent;
    while parent != 0 {
        let above = elements(tree).flatten().find(|other| other.id == parent)?;
        if is_container(above.role) {
            return Some(above);
        }
        parent = above.parent;
    }
    None
}
