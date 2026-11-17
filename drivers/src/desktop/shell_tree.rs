//! The shell's own element tree (D605; docs/ux/adaptive-shell-v0.5.md section 9): what
//! `wm show shell` lists and `wm act shell` presses, in the phone and tablet forms. The window
//! server answers it itself, as window [`SHELL_WINDOW`]; no app is asked.

use super::catalogue::AppId;
use super::form::Form;
use super::network::NetworkState;
use super::protocol::EventSink;
use super::{Action, Desktop, MAX_WINDOWS};
use crate::display::{Damage, Rect};
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_element_tree::{
    Action as ElementAction, Bounds, Element, Outcome, Role, TreeWriter, flags,
};

/// The window number `GET_TREE` and `ACT_ON_ELEMENT` take for the shell itself.
pub const SHELL_WINDOW: u32 = 0;

const ROOT: u32 = 1;
const STATUS_BAR: u32 = 2;
const CLOCK: u32 = 3;
const TITLE: u32 = 4;
const NETWORK: u32 = 5;
const BATTERY: u32 = 6;
const AGENT: u32 = 7;
const BACK: u32 = 8;
const HOME: u32 = 9;
const APPS_BUTTON: u32 = 10;
const SEARCH: u32 = 11;
const APPS_LIST: u32 = 12;
const UPDATES: u32 = 13;
const FIRST_APP: u32 = 100;
const VALUE_BYTES: usize = 64;

fn bounds(rect: Rect) -> Bounds {
    Bounds::new(
        rect.x as i32,
        rect.y as i32,
        rect.width as i32,
        rect.height as i32,
    )
}

fn network_word(state: Option<NetworkState>) -> &'static str {
    match state {
        None => "none",
        Some(NetworkState::CableUnplugged) => "disconnected",
        Some(NetworkState::GettingAddress) => "connecting",
        Some(NetworkState::NoAddress) => "no address",
        Some(NetworkState::Connected(_)) => "connected",
    }
}

struct Value {
    bytes: [u8; VALUE_BYTES],
    length: usize,
}

impl Value {
    fn new() -> Self {
        Self {
            bytes: [0; VALUE_BYTES],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Value {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = VALUE_BYTES - self.length;
        let mut cut = text.len().min(room);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.bytes[self.length..self.length + cut].copy_from_slice(&text.as_bytes()[..cut]);
        self.length += cut;
        Ok(())
    }
}

impl Desktop {
    fn shell_has_tree(&self) -> bool {
        matches!(self.form, Form::Phone | Form::Tablet | Form::Desktop)
    }

    /// The bar's `Updates` chip (docs/ux/components-v0.5.md section 7): a button valued with
    /// the count of updates, while it shows.
    fn describe_updates_chip(&self, tree: &mut TreeWriter<'_>) {
        let Some(chip) = self.updates_chip() else {
            return;
        };
        let mut count = Value::new();
        let _ = write!(count, "{}", self.updates_count());
        tree.push(
            &Element::new(UPDATES, ROOT, Role::Button, bounds(chip))
                .named(b"Updates")
                .valued(count.as_bytes()),
        );
    }

    /// The shell's element tree in an element tree's layout, or nothing outside the forms
    /// that have one yet. The desktop form lists only its `Updates` chip so far.
    pub fn shell_tree(&self) -> Vec<u8> {
        if !self.shell_has_tree() {
            return Vec::new();
        }
        let mut bytes = alloc::vec![0u8; eggos_element_tree::MAX_TREE_BYTES];
        let mut tree = TreeWriter::new(&mut bytes);
        tree.push(&Element::new(ROOT, 0, Role::Window, bounds(self.screen())).named(b"Shell"));
        if self.form == Form::Desktop {
            self.describe_updates_chip(&mut tree);
            let length = tree.bytes().len();
            bytes.truncate(length);
            return bytes;
        }
        if let Some(bar) = self.layout.status_bar {
            let expanded = match self.phone.shade {
                true => flags::EXPANDED,
                false => flags::EXPANDABLE,
            };
            tree.push(
                &Element::new(STATUS_BAR, ROOT, Role::Button, bounds(bar))
                    .named(b"Status bar")
                    .flagged(expanded),
            );
            let title = self
                .phone_shown_window()
                .map(|slot| *self.window_at(slot));
            let battery = self.battery_indicator_name().unwrap_or("not reported");
            let agent = match self.agent_acting() {
                true => "acting",
                false => "idle",
            };
            let lines: [(u32, &[u8], &[u8], bool); 5] = [
                (CLOCK, b"Clock", self.clock_text().as_bytes(), true),
                (
                    TITLE,
                    b"Title",
                    title.as_ref().map_or(&b""[..], |window| window.title().as_bytes()),
                    true,
                ),
                (
                    NETWORK,
                    b"Network",
                    network_word(self.network_state()).as_bytes(),
                    true,
                ),
                (BATTERY, b"Battery", battery.as_bytes(), true),
                (AGENT, b"Agent", agent.as_bytes(), self.agent_connected()),
            ];
            for (id, name, value, listed) in lines {
                if listed {
                    tree.push(
                        &Element::new(id, STATUS_BAR, Role::Text, bounds(bar))
                            .named(name)
                            .valued(value),
                    );
                }
            }
        }
        let buttons = [(BACK, b"Back", 0), (HOME, b"Home", 1), (APPS_BUTTON, b"Apps", 2)];
        for (id, name, zone) in buttons {
            let Some(rect) = self.navigation_zone(zone) else {
                continue;
            };
            let disabled = id == BACK && !self.phone_can_go_back();
            let element = Element::new(id, ROOT, Role::Button, bounds(rect)).named(&name[..]);
            tree.push(&match disabled {
                true => element.flagged(flags::DISABLED),
                false => element,
            });
        }
        if self.phone_shown_window().is_none() {
            self.describe_home(&mut tree);
        }
        let length = tree.bytes().len();
        bytes.truncate(length);
        bytes
    }

    fn describe_home(&self, tree: &mut TreeWriter<'_>) {
        let field = self.search_field();
        let focused = match self.phone.search_focused {
            true => flags::FOCUSED,
            false => 0,
        };
        tree.push(
            &Element::new(SEARCH, ROOT, Role::TextField, bounds(field))
                .named(b"Search apps")
                .valued(self.phone.query().as_bytes())
                .flagged(flags::FOCUSABLE | focused),
        );
        let count = self.catalogue.desktop_apps().count();
        let mut listed = Value::new();
        let _ = write!(listed, "{count} apps");
        tree.push(
            &Element::new(APPS_LIST, ROOT, Role::List, bounds(self.grid_area()))
                .named(b"Apps")
                .valued(listed.as_bytes()),
        );
        for (index, app) in self.catalogue.desktop_apps().enumerate() {
            let mut windows = [0u8; MAX_WINDOWS];
            let running: &[u8] = match self.app_windows(app, &mut windows) {
                0 => b"",
                _ => b"running",
            };
            tree.push(
                &Element::new(
                    FIRST_APP + index as u32,
                    APPS_LIST,
                    Role::Button,
                    bounds(self.cell(index)),
                )
                .named(self.catalogue.name(app).as_bytes())
                .valued(running),
            );
        }
    }

    /// Acts on the shell's element `element` (`wm act shell`): `invoke` presses a button,
    /// `set-text` types into the search, `focus` focuses it. Returns how it went and what the
    /// kernel must do next (an app to start).
    pub fn act_on_shell(
        &mut self,
        element: u32,
        action: ElementAction,
        text: &str,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> (Outcome, Action) {
        if !self.shell_has_tree() {
            return (Outcome::NoSuchElement, Action::None);
        }
        damage.add(self.screen());
        if self.form == Form::Desktop {
            return match (element, action) {
                (UPDATES, ElementAction::Invoke) if self.updates_chip().is_some() => (
                    Outcome::Done,
                    Action::OpenSettingsPane(super::SettingsPane::Components),
                ),
                (ROOT, _) => (Outcome::NotSupported, Action::None),
                _ => (Outcome::NoSuchElement, Action::None),
            };
        }
        let home = self.phone_shown_window().is_none();
        match (element, action) {
            (STATUS_BAR, ElementAction::Invoke) => {
                let open = !self.phone.shade;
                self.close_phone_overlays();
                self.phone.shade = open;
                (Outcome::Done, Action::None)
            }
            (BACK, ElementAction::Invoke) if self.phone_can_go_back() => {
                self.phone_back(damage, events);
                (Outcome::Done, Action::None)
            }
            (BACK, ElementAction::Invoke) => (Outcome::Disabled, Action::None),
            (HOME, ElementAction::Invoke) => {
                self.phone_home(damage, events);
                (Outcome::Done, Action::None)
            }
            (APPS_BUTTON, ElementAction::Invoke) => {
                let open = !self.phone.switcher;
                self.close_phone_overlays();
                self.phone.switcher = open;
                (Outcome::Done, Action::None)
            }
            (SEARCH, ElementAction::Focus) if home => {
                self.phone.search_focused = true;
                (Outcome::Done, Action::None)
            }
            (SEARCH, ElementAction::SetText) if home => {
                let mut length = 0;
                for &byte in text.as_bytes().iter().filter(|byte| (0x20..0x7f).contains(*byte)) {
                    if length == self.phone.query.len() {
                        break;
                    }
                    self.phone.query[length] = byte;
                    length += 1;
                }
                self.phone.query_length = length;
                self.phone.search_focused = true;
                (Outcome::Done, Action::None)
            }
            (id, ElementAction::Invoke) if home && id >= FIRST_APP => {
                let app: Option<AppId> = self
                    .catalogue
                    .desktop_apps()
                    .nth((id - FIRST_APP) as usize);
                match app {
                    Some(app) => (Outcome::Done, self.phone_open_app(app, damage, events)),
                    None => (Outcome::NoSuchElement, Action::None),
                }
            }
            (CLOCK | TITLE | NETWORK | BATTERY | AGENT | APPS_LIST | ROOT, _) => {
                (Outcome::NotSupported, Action::None)
            }
            _ => (Outcome::NoSuchElement, Action::None),
        }
    }
}
