//! The Accessibility pane's key rows (D205, docs/ux/first-run-accessibility-v0.3.md section 2):
//! sticky keys, slow keys and the delay a key must be held, and full keyboard navigation,
//! always on. Each change is one [`Request::ChangeKeys`] the program saves, then applies.

use crate::agents_pane::show_line;
use crate::facts::Facts;
use crate::input_panes::Reply;
use crate::keys::KeysChange;
use crate::settings_window::{CAPACITY, Request};
use eggos_runtime::errno::Errno;
use eggos_settings_file::KeyAccessibility;
use eggos_toolkit::colour;
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Horizontal, Kind, Length, Place};

const PADDING: i32 = 16;
const SLIDER_WIDTH: i32 = 216;
const NO_DISK: &str = "No disk attached: this choice lasts until power-off.";

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn label(
    tree: &mut WidgetTree<CAPACITY>,
    pane: WidgetId,
    x: i32,
    y: i32,
    words: &str,
    ink: u32,
) -> WidgetId {
    add(
        tree,
        pane,
        Place::at(x, y, Length::Content, Length::Content),
        Kind::label(words, ink),
    )
}

fn meaning(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, y: i32, words: &str, ink: u32) {
    add(
        tree,
        pane,
        Place::right(PADDING, y),
        Kind::label(words, ink),
    );
}

fn checkbox(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, y: i32, words: &str) -> WidgetId {
    add(
        tree,
        pane,
        Place::at(PADDING, y, Length::Content, Length::Px(CONTROL_HEIGHT)),
        Kind::Checkbox {
            text: Text::from(words),
            checked: false,
        },
    )
}

/// The Accessibility pane's key widgets.
#[derive(Debug)]
pub struct AccessibilityPane {
    sticky: WidgetId,
    slow: WidgetId,
    delay: WidgetId,
    status: WidgetId,
}

impl AccessibilityPane {
    /// Builds the pane's rows in `pane`, under its heading, showing `keys`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, keys: KeyAccessibility) -> Self {
        let sticky = checkbox(tree, pane, 150, "Sticky keys");
        meaning(
            tree,
            pane,
            156,
            "Tap a modifier, then the key; twice locks it.",
            colour::TEXT_DIM,
        );
        let slow = checkbox(tree, pane, 190, "Slow keys");
        meaning(
            tree,
            pane,
            196,
            "A key counts only when held this long.",
            colour::TEXT_DIM,
        );
        label(tree, pane, 40, 236, "Hold a key for", colour::TEXT);
        let delay = add(
            tree,
            pane,
            Place {
                horizontal: Horizontal::Right(PADDING),
                top: 230,
                width: Length::Px(SLIDER_WIDTH),
                height: Length::Px(CONTROL_HEIGHT),
            },
            Kind::slider(
                "Hold a key for",
                100,
                1000,
                50,
                keys.slow_keys_ms as i32,
                " ms",
            ),
        );
        label(
            tree,
            pane,
            PADDING,
            276,
            "Full keyboard navigation",
            colour::TEXT,
        );
        meaning(tree, pane, 276, "Always on", colour::TEXT);
        label(
            tree,
            pane,
            PADDING,
            296,
            "Tab reaches every control; Ctrl+Alt+Tab reaches the bar.",
            colour::TEXT_DIM,
        );
        let status = label(tree, pane, PADDING, 316, "", colour::TEXT_DIM);
        let accessibility = Self {
            sticky,
            slow,
            delay,
            status,
        };
        accessibility.show(tree, keys);
        tree.set_shown(status, false);
        accessibility
    }

    /// Shows `keys` in the controls.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, keys: KeyAccessibility) {
        tree.set_selected(self.sticky, usize::from(keys.sticky_keys));
        tree.set_selected(self.slow, usize::from(keys.slow_keys));
        tree.set_value(self.delay, keys.slow_keys_ms as i32);
        tree.set_enabled(self.delay, keys.slow_keys);
    }

    /// Shows the pane afresh: the controls at `facts`, the status line at its "shown" state.
    pub fn reset(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        self.show(tree, facts.keys);
        match facts.on_disk {
            true => tree.set_shown(self.status, false),
            false => {
                show_line(tree, self.status, NO_DISK, colour::TEXT_DIM);
                tree.set_shown(self.status, true);
            }
        }
    }

    /// How a change went: `keys` are those in force after it.
    pub fn changed(
        &self,
        tree: &mut WidgetTree<CAPACITY>,
        keys: KeyAccessibility,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        self.show(tree, keys);
        let line = match (saved, applied) {
            (Err(Errno::EPERM | Errno::EACCES), _) => {
                "Not allowed: this user may not change the keyboard settings."
            }
            (Err(_), _) => "No disk attached: the keyboard settings could not be saved.",
            (Ok(()), Err(_)) => "Saved for the next boot, but the window server did not take it.",
            (Ok(()), Ok(())) => return,
        };
        show_line(tree, self.status, line, colour::TEXT);
        tree.set_shown(self.status, true);
    }

    /// The controls' state: sticky keys, slow keys, the delay and whether it is enabled.
    pub fn state(&self, tree: &WidgetTree<CAPACITY>) -> (bool, bool, Option<i32>, bool) {
        (
            tree.selected(self.sticky) == Some(1),
            tree.selected(self.slow) == Some(1),
            tree.value(self.delay),
            tree.is_enabled(self.delay),
        )
    }

    /// Acts on a response, the keys in force being `keys`.
    pub fn respond(&self, response: Response, keys: KeyAccessibility) -> Reply {
        let change = match response {
            Response::Toggled(id, on) if id == self.sticky => KeysChange::StickyKeys(on),
            Response::Toggled(id, on) if id == self.slow => KeysChange::SlowKeys(on),
            Response::Adjusted(id, value) if id == self.delay => {
                KeysChange::SlowKeysMs(value as u32)
            }
            _ => return Reply::NotMine,
        };
        let unchanged = change.applied_to(keys) == keys;
        Reply::Done((!unchanged).then_some(Request::ChangeKeys(change)))
    }
}
