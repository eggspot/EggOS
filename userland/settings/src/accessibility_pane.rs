//! The Accessibility pane (D205, D223, docs/ux/first-run-accessibility-v0.3.md section 2):
//! high contrast, reduce motion, sticky keys, slow keys and the delay a key must be held, and
//! full keyboard navigation, always on. Each change is one [`Request::ChangeDisplay`] or
//! [`Request::ChangeKeys`] the program saves, then applies.

use crate::agents_pane::show_line;
use crate::facts::Facts;
use crate::input_panes::Reply;
use crate::keys::{DisplayChange, KeysChange};
use crate::settings_window::{CAPACITY, Request};
use eggos_runtime::errno::Errno;
use eggos_settings_file::{DisplayAccessibility, KeyAccessibility};
use eggos_toolkit::theme::Token;
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
    ink: Token,
) -> WidgetId {
    add(
        tree,
        pane,
        Place::at(x, y, Length::Content, Length::Content),
        Kind::label(words, ink),
    )
}

fn meaning(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, y: i32, words: &str, ink: Token) {
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

/// What the pane's controls show: the display's and the keys' accessibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub display: DisplayAccessibility,
    pub keys: KeyAccessibility,
}

impl Shown {
    /// What `facts` hold.
    pub fn of(facts: &Facts) -> Self {
        Self {
            display: facts.display,
            keys: facts.keys,
        }
    }
}

/// The Accessibility pane's widgets.
#[derive(Debug)]
pub struct AccessibilityPane {
    contrast: WidgetId,
    motion: WidgetId,
    sticky: WidgetId,
    slow: WidgetId,
    delay: WidgetId,
    status: WidgetId,
}

impl AccessibilityPane {
    /// Builds the pane's rows in `pane`, under its heading, showing `shown`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, shown: Shown) -> Self {
        let contrast = checkbox(tree, pane, 70, "High contrast");
        meaning(
            tree,
            pane,
            76,
            "White on black, every edge drawn.",
            Token::TextDim,
        );
        let motion = checkbox(tree, pane, 110, "Reduce motion");
        meaning(tree, pane, 116, "No blinking, no animation.", Token::TextDim);
        let sticky = checkbox(tree, pane, 150, "Sticky keys");
        meaning(
            tree,
            pane,
            156,
            "Tap a modifier, then the key; twice locks it.",
            Token::TextDim,
        );
        let slow = checkbox(tree, pane, 190, "Slow keys");
        meaning(
            tree,
            pane,
            196,
            "A key counts only when held this long.",
            Token::TextDim,
        );
        label(tree, pane, 40, 236, "Hold a key for", Token::Text);
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
                shown.keys.slow_keys_ms as i32,
                " ms",
            ),
        );
        label(
            tree,
            pane,
            PADDING,
            276,
            "Full keyboard navigation",
            Token::Text,
        );
        meaning(tree, pane, 276, "Always on", Token::Text);
        label(
            tree,
            pane,
            PADDING,
            296,
            "Tab reaches every control; Ctrl+Alt+Tab reaches the bar.",
            Token::TextDim,
        );
        let status = label(tree, pane, PADDING, 316, "", Token::TextDim);
        let accessibility = Self {
            contrast,
            motion,
            sticky,
            slow,
            delay,
            status,
        };
        accessibility.show(tree, shown);
        tree.set_shown(status, false);
        accessibility
    }

    /// Shows `shown` in the controls.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, shown: Shown) {
        let Shown { display, keys } = shown;
        tree.set_selected(self.contrast, usize::from(display.high_contrast));
        tree.set_selected(self.motion, usize::from(display.reduce_motion));
        tree.set_selected(self.sticky, usize::from(keys.sticky_keys));
        tree.set_selected(self.slow, usize::from(keys.slow_keys));
        tree.set_value(self.delay, keys.slow_keys_ms as i32);
        tree.set_enabled(self.delay, keys.slow_keys);
    }

    /// Shows the pane afresh: the controls at `facts`, the status line at its "shown" state.
    pub fn reset(&self, tree: &mut WidgetTree<CAPACITY>, facts: &Facts) {
        self.show(tree, Shown::of(facts));
        match facts.on_disk {
            true => tree.set_shown(self.status, false),
            false => {
                show_line(tree, self.status, NO_DISK, Token::TextDim);
                tree.set_shown(self.status, true);
            }
        }
    }

    /// How a change went: `shown` is what is in force after it.
    pub fn changed(
        &self,
        tree: &mut WidgetTree<CAPACITY>,
        shown: Shown,
        saved: Result<(), Errno>,
        applied: Result<(), Errno>,
    ) {
        self.show(tree, shown);
        let line = match (saved, applied) {
            (Err(Errno::EPERM | Errno::EACCES), _) => {
                "Not allowed: this user may not change the accessibility settings."
            }
            (Err(_), _) => "No disk attached: the accessibility settings could not be saved.",
            (Ok(()), Err(_)) => "Saved for the next boot, but the window server did not take it.",
            (Ok(()), Ok(())) => return,
        };
        show_line(tree, self.status, line, Token::Text);
        tree.set_shown(self.status, true);
    }

    /// Whether the high contrast and reduce motion boxes are checked.
    pub fn display_state(&self, tree: &WidgetTree<CAPACITY>) -> (bool, bool) {
        (
            tree.selected(self.contrast) == Some(1),
            tree.selected(self.motion) == Some(1),
        )
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

    /// Acts on a response, what is in force being `shown`.
    pub fn respond(&self, response: Response, shown: Shown) -> Reply {
        let display = match response {
            Response::Toggled(id, on) if id == self.contrast => {
                Some(DisplayChange::HighContrast(on))
            }
            Response::Toggled(id, on) if id == self.motion => {
                Some(DisplayChange::ReduceMotion(on))
            }
            _ => None,
        };
        if let Some(change) = display {
            let unchanged = change.applied_to(shown.display) == shown.display;
            return Reply::Done((!unchanged).then_some(Request::ChangeDisplay(change)));
        }
        let keys = shown.keys;
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
