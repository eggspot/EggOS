//! The Files window as an element tree for an agent (D161, docs/windows.md "Element trees"):
//! the sidebar's places, the path field and the folder's entries as list items, each with its
//! name and whether it is a folder, a file, a link to either or a broken link. Invoking an entry
//! opens it as a double-click does, selecting it selects it, and invoking a place goes there.

use super::{FilesWindow, Focus};
use crate::kind::Link;
use crate::names;
use eggos_app::files::Files;
use eggos_toolkit::elements::{Action, Bounds, Element, Outcome, Role, TreeWriter, flags};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::tree::ROOT_ELEMENT;
use eggos_toolkit::widget;

/// The sidebar's list of places.
pub const PLACES_ELEMENT: u32 = 10;
/// The path field.
pub const PATH_ELEMENT: u32 = 20;
/// The list of the folder's entries.
pub const ENTRIES_ELEMENT: u32 = 30;
/// Place `i` is element `FIRST_PLACE_ELEMENT + i`.
pub const FIRST_PLACE_ELEMENT: u32 = 11;
/// Entry `i` is element `FIRST_ENTRY_ELEMENT + i`.
pub const FIRST_ENTRY_ELEMENT: u32 = 1000;

fn bounds(rect: Rect) -> Bounds {
    Bounds::new(rect.x, rect.y, rect.width, rect.height)
}

fn when(condition: bool, flag: u16) -> u16 {
    if condition { flag } else { 0 }
}

impl FilesWindow {
    fn focused_on(&self, focus: Focus) -> u16 {
        when(self.window_focused && self.focus == focus, flags::FOCUSED)
    }

    /// Writes the places, the path field and the entries below [`ROOT_ELEMENT`]; stops when
    /// `tree` is full.
    pub fn describe(&self, tree: &mut TreeWriter<'_>) {
        let places = Element::new(
            PLACES_ELEMENT,
            ROOT_ELEMENT,
            Role::List,
            bounds(self.sidebar()),
        )
        .named(b"Places")
        .flagged(flags::FOCUSABLE | self.focused_on(Focus::Sidebar));
        if !tree.push(&places) {
            return;
        }
        let current = self.current_place();
        for (index, place) in self.places().iter().enumerate() {
            let item = Element::new(
                FIRST_PLACE_ELEMENT + index as u32,
                PLACES_ELEMENT,
                Role::ListItem,
                bounds(self.place_rect(index)),
            )
            .named(place.label(self.windows_words).as_bytes())
            .flagged(when(current == Some(index), flags::SELECTED));
            if !tree.push(&item) {
                return;
            }
        }
        let path = Element::new(
            PATH_ELEMENT,
            ROOT_ELEMENT,
            Role::TextField,
            bounds(self.path_field_rect()),
        )
        .named(b"Path")
        .valued(self.path_field.text.as_str().as_bytes())
        .flagged(flags::FOCUSABLE | self.focused_on(Focus::Path));
        let entries = Element::new(
            ENTRIES_ELEMENT,
            ROOT_ELEMENT,
            Role::List,
            bounds(self.list()),
        )
        .named(b"Entries")
        .flagged(flags::FOCUSABLE | self.focused_on(Focus::List));
        if !tree.push(&path) || !tree.push(&entries) {
            return;
        }
        let mut shown = [0u8; names::NAME_CAPACITY * 2];
        for index in 0..self.listing.len() {
            let Some(entry) = self.listing.get(index) else {
                break;
            };
            let name = names::display(self.listing.name(index), &mut shown);
            let kind: &[u8] = match (entry.link, entry.kind.is_folder()) {
                (Link::None, true) => b"folder",
                (Link::None, false) => b"file",
                (Link::Sound, true) => b"link to folder",
                (Link::Sound, false) => b"link to file",
                (Link::Broken, _) => b"broken link",
            };
            let item = Element::new(
                FIRST_ENTRY_ELEMENT + index as u32,
                ENTRIES_ELEMENT,
                Role::ListItem,
                bounds(self.row_rect(index)),
            )
            .named(name.as_bytes())
            .valued(kind)
            .flagged(when(self.selected == Some(index), flags::SELECTED));
            if !tree.push(&item) {
                return;
            }
        }
    }

    /// Does `action` to `element` for an agent, as a person's clicks and keys would, with `text` for
    /// [`Action::SetText`] (the path field takes it as typed; Enter goes there).
    pub fn act(
        &mut self,
        files: &mut impl Files,
        element: u32,
        action: Action,
        text: &str,
    ) -> Outcome {
        let place = element
            .checked_sub(FIRST_PLACE_ELEMENT)
            .map(|index| index as usize)
            .filter(|&index| element < PATH_ELEMENT && index < self.places().len());
        let entry = element
            .checked_sub(FIRST_ENTRY_ELEMENT)
            .map(|index| index as usize)
            .filter(|&index| index < self.listing.len());
        let known = matches!(element, PLACES_ELEMENT | PATH_ELEMENT | ENTRIES_ELEMENT);
        if place.is_none() && entry.is_none() && !known {
            return Outcome::NoSuchElement;
        }
        if self.panel.is_some() || self.rename.is_some() {
            return Outcome::Disabled;
        }
        self.menu = None;
        match (action, place, entry, element) {
            (Action::Invoke | Action::Select, Some(index), _, _) => {
                self.go_to_place(files, index);
                Outcome::Done
            }
            (Action::Select, _, Some(index), _) => {
                self.focus = Focus::List;
                self.select(Some(index));
                Outcome::Done
            }
            (Action::Invoke, _, Some(index), _) => {
                self.focus = Focus::List;
                self.select(Some(index));
                self.open(files, index);
                Outcome::Done
            }
            (Action::Focus, _, _, PATH_ELEMENT) => {
                self.focus_path();
                self.damage_all();
                Outcome::Done
            }
            (Action::SetText, _, _, PATH_ELEMENT) => {
                self.focus_path();
                self.path_field.text.set(text);
                self.path_field.caret = self.path_field.text.len();
                let rect = self.path_field_rect();
                widget::keep_caret_visible(&mut self.path_field, rect);
                self.damage_all();
                Outcome::Done
            }
            (Action::Focus, _, _, PLACES_ELEMENT | ENTRIES_ELEMENT) => {
                self.focus = match element {
                    PLACES_ELEMENT => Focus::Sidebar,
                    _ => Focus::List,
                };
                self.damage_all();
                Outcome::Done
            }
            _ => Outcome::NotSupported,
        }
    }
}
