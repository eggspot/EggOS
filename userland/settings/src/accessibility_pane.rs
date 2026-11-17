//! The Accessibility pane (D205, D224, D602, D603; docs/ux/first-run-accessibility-v0.3.md
//! section 2 and docs/ux/accessibility-v0.4.md section 4.1): under `Seeing`, high contrast,
//! reduce motion, the screen reader, zoom and its level, and the pointer's size; under `Keys and
//! pointer`, sticky keys, slow keys and their delay, bounce keys and theirs, toggle keys, mouse
//! keys and their top speed; and full keyboard navigation, always on. Each change is one
//! [`Request::ChangeDisplay`], [`Request::ChangeReader`] or [`Request::ChangeKeys`] the program
//! saves, then applies.

use crate::agents_pane::show_line;
use crate::facts::Facts;
use crate::input_panes::Reply;
use crate::keys::{DisplayChange, KeysChange, ReaderChange};
use crate::settings_window::{CAPACITY, Request};
use alloc::vec::Vec;
use eggos_runtime::errno::Errno;
use eggos_settings_file::{
    DisplayAccessibility, KeyAccessibility, POINTER_SIZES, ScreenReaderSettings, ZOOM_LEVELS,
};
use eggos_toolkit::text::Text;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{CONTROL_HEIGHT, Horizontal, Kind, Length, Place};

const PADDING: i32 = 16;
const INDENT: i32 = 40;
const FIRST_ROW: i32 = 64;
const ROW: i32 = 40;
const TEXT_IN_ROW: i32 = 12;
const CONTROL_IN_ROW: i32 = 6;
const SLIDER_WIDTH: i32 = 216;
const TOP_SPEED_WIDTH: i32 = 224;
const DROPDOWN_WIDTH: i32 = 160;
const TOUCH_CONTROL: i32 = 48;
const LINE: i32 = 20;
/// The pane's height on the desktop: its rows from 64, 40 apart, the navigation lines and the
/// status line.
pub const ACCESSIBILITY_PANE_HEIGHT: i32 = 788;
const NO_DISK: &str = "No disk attached: this choice lasts until power-off.";
const LEVEL_OPTIONS: [&str; 5] = ["2x", "3x", "4x", "6x", "8x"];
const POINTER_SIZE_OPTIONS: [&str; 4] =
    ["Normal (1x)", "Large (1.5x)", "Larger (2x)", "Largest (3x)"];

fn add(tree: &mut WidgetTree<CAPACITY>, parent: WidgetId, place: Place, kind: Kind) -> WidgetId {
    tree.add(Some(parent), place, kind)
        .expect("the Settings window fits its tree")
}

fn text_at(x: i32, y: i32) -> Place {
    Place::at(x, y, Length::Content, Length::Content)
}

fn from_right(top: i32, width: i32) -> Place {
    Place {
        horizontal: Horizontal::Right(PADDING),
        top,
        width: Length::Px(width),
        height: Length::Px(CONTROL_HEIGHT),
    }
}

/// What the pane's controls show: the display's and the keys' accessibility and the screen
/// reader's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub display: DisplayAccessibility,
    pub keys: KeyAccessibility,
    pub reader: ScreenReaderSettings,
}

impl Shown {
    /// What `facts` hold.
    pub fn of(facts: &Facts) -> Self {
        Self {
            display: facts.display,
            keys: facts.keys,
            reader: facts.screen_reader,
        }
    }
}

/// One row of the pane, in order, as it is placed on the desktop and on a narrow page.
#[derive(Debug, Clone, Copy)]
enum Row {
    /// A sub-heading: `Seeing`, `Keys and pointer`.
    Heading(WidgetId),
    /// A checkbox with its meaning right-aligned beside it, or under it on a narrow page.
    Check { check: WidgetId, meaning: WidgetId },
    /// A label with its control right-aligned, `width` wide (a dropdown or a slider); the
    /// label indented when the row belongs to the checkbox above.
    Control {
        label: WidgetId,
        control: WidgetId,
        width: i32,
        indented: bool,
    },
}

/// The Accessibility pane's widgets.
#[derive(Debug)]
pub struct AccessibilityPane {
    rows: Vec<Row>,
    navigation: [WidgetId; 3],
    contrast: WidgetId,
    motion: WidgetId,
    reader: WidgetId,
    zoom: WidgetId,
    level: WidgetId,
    pointer_size: WidgetId,
    sticky: WidgetId,
    slow: WidgetId,
    delay: WidgetId,
    bounce: WidgetId,
    bounce_delay: WidgetId,
    toggle: WidgetId,
    mouse_keys: WidgetId,
    top_speed: WidgetId,
    status: WidgetId,
}

struct Builder<'a> {
    tree: &'a mut WidgetTree<CAPACITY>,
    pane: WidgetId,
    rows: Vec<Row>,
}

impl Builder<'_> {
    fn heading(&mut self, words: &str) {
        let id = add(
            self.tree,
            self.pane,
            text_at(PADDING, FIRST_ROW),
            Kind::label(words, Token::Text),
        );
        self.rows.push(Row::Heading(id));
    }

    fn check(&mut self, words: &str, meaning: &str) -> WidgetId {
        let check = add(
            self.tree,
            self.pane,
            Place::at(PADDING, FIRST_ROW, Length::Content, Length::Px(CONTROL_HEIGHT)),
            Kind::Checkbox {
                text: Text::from(words),
                checked: false,
            },
        );
        let meaning = add(
            self.tree,
            self.pane,
            Place::right(PADDING, FIRST_ROW),
            Kind::label(meaning, Token::TextDim),
        );
        self.rows.push(Row::Check { check, meaning });
        check
    }

    fn control(&mut self, words: &str, kind: Kind, width: i32, indented: bool) -> WidgetId {
        let label = add(
            self.tree,
            self.pane,
            text_at(PADDING, FIRST_ROW),
            Kind::label(words, Token::Text),
        );
        let control = add(self.tree, self.pane, from_right(FIRST_ROW, width), kind);
        self.rows.push(Row::Control {
            label,
            control,
            width,
            indented,
        });
        control
    }
}

impl AccessibilityPane {
    /// The controls that set `key` on this pane, for a lock (D666).
    pub fn controls(&self, key: crate::keys::Key) -> &[WidgetId] {
        use crate::keys::Key;
        match key {
            Key::KeyboardNavigation => &self.navigation,
            Key::HighContrast => core::slice::from_ref(&self.contrast),
            Key::ReduceMotion => core::slice::from_ref(&self.motion),
            Key::ScreenReader => core::slice::from_ref(&self.reader),
            Key::Zoom => core::slice::from_ref(&self.zoom),
            Key::ZoomLevel => core::slice::from_ref(&self.level),
            Key::PointerSize => core::slice::from_ref(&self.pointer_size),
            Key::StickyKeys => core::slice::from_ref(&self.sticky),
            Key::SlowKeys => core::slice::from_ref(&self.slow),
            Key::SlowKeysMs => core::slice::from_ref(&self.delay),
            Key::BounceKeys => core::slice::from_ref(&self.bounce),
            Key::BounceKeysMs => core::slice::from_ref(&self.bounce_delay),
            Key::ToggleKeys => core::slice::from_ref(&self.toggle),
            Key::MouseKeys => core::slice::from_ref(&self.mouse_keys),
            Key::MouseKeysSpeed => core::slice::from_ref(&self.top_speed),
            _ => &[],
        }
    }

    /// Builds the pane's rows in `pane`, under its heading, showing `shown`.
    pub fn build(tree: &mut WidgetTree<CAPACITY>, pane: WidgetId, shown: Shown) -> Self {
        let mut rows = Builder {
            tree,
            pane,
            rows: Vec::new(),
        };
        rows.heading("Seeing");
        let contrast = rows.check("High contrast", "White on black, every edge drawn.");
        let motion = rows.check("Reduce motion", "No blinking, no animation.");
        let reader = rows.check("Screen reader", "Shows what it reads in a caption panel.");
        let zoom = rows.check("Zoom", "Magnifies the screen around the focus.");
        let level = rows.control(
            "Level",
            Kind::dropdown("Level", &LEVEL_OPTIONS, 0),
            DROPDOWN_WIDTH,
            true,
        );
        let pointer_size = rows.control(
            "Pointer size",
            Kind::dropdown("Pointer size", &POINTER_SIZE_OPTIONS, 0),
            DROPDOWN_WIDTH,
            false,
        );
        rows.heading("Keys and pointer");
        let sticky = rows.check("Sticky keys", "Tap a modifier, then the key; twice locks it.");
        let slow = rows.check("Slow keys", "A key counts only when held this long.");
        let delay = rows.control(
            "Hold a key for",
            Kind::slider(
                "Hold a key for",
                100,
                1000,
                50,
                shown.keys.slow_keys_ms as i32,
                " ms",
            ),
            SLIDER_WIDTH,
            true,
        );
        let bounce = rows.check("Bounce keys", "A key pressed again this soon is ignored.");
        let bounce_delay = rows.control(
            "Ignore a repeat within",
            Kind::slider(
                "Ignore a repeat within",
                100,
                1000,
                50,
                shown.keys.bounce_keys_ms as i32,
                " ms",
            ),
            SLIDER_WIDTH,
            true,
        );
        let toggle = rows.check("Toggle keys", "Says when Caps, Num or Scroll Lock change.");
        let mouse_keys = rows.check("Mouse keys", "The numeric keypad moves the pointer.");
        let top_speed = rows.control(
            "Top speed",
            Kind::slider(
                "Top speed",
                100,
                1000,
                50,
                shown.keys.mouse_keys_speed as i32,
                " px/s",
            ),
            TOP_SPEED_WIDTH,
            true,
        );
        let Builder { tree, rows, .. } = rows;
        let navigation = [
            add(
                tree,
                pane,
                text_at(PADDING, FIRST_ROW),
                Kind::label("Full keyboard navigation", Token::Text),
            ),
            add(
                tree,
                pane,
                Place::right(PADDING, FIRST_ROW),
                Kind::label("Always on", Token::Text),
            ),
            add(
                tree,
                pane,
                text_at(PADDING, FIRST_ROW),
                Kind::label(
                    "Tab reaches every control; Ctrl+Alt+Tab reaches the bar.",
                    Token::TextDim,
                ),
            ),
        ];
        let status = add(tree, pane, text_at(PADDING, FIRST_ROW), Kind::label("", Token::TextDim));
        let accessibility = Self {
            rows,
            navigation,
            contrast,
            motion,
            reader,
            zoom,
            level,
            pointer_size,
            sticky,
            slow,
            delay,
            bounce,
            bounce_delay,
            toggle,
            mouse_keys,
            top_speed,
            status,
        };
        accessibility.reflow(tree, None);
        accessibility.show(tree, shown);
        tree.set_shown(status, false);
        accessibility
    }

    /// Places the rows on the desktop (`None`: the 40 px grid from 64, each control
    /// right-aligned), or in one column `content` wide on a narrow page
    /// (settings-forms-v0.5.md 2, R8 and R2: each check box a 48 px row with its meaning under
    /// it, each dropdown and slider under its label); returns the pane's height.
    pub fn reflow(&self, tree: &mut WidgetTree<CAPACITY>, content: Option<i32>) -> i32 {
        let mut y = FIRST_ROW;
        for &row in &self.rows {
            y = match (row, content) {
                (Row::Heading(id), _) => {
                    tree.set_place(id, text_at(PADDING, y + TEXT_IN_ROW));
                    y + ROW
                }
                (Row::Check { check, meaning }, None) => {
                    tree.set_place(
                        check,
                        Place::at(
                            PADDING,
                            y + CONTROL_IN_ROW,
                            Length::Content,
                            Length::Px(CONTROL_HEIGHT),
                        ),
                    );
                    tree.set_place(meaning, Place::right(PADDING, y + TEXT_IN_ROW));
                    y + ROW
                }
                (Row::Check { check, meaning }, Some(c)) => {
                    tree.set_place(
                        check,
                        Place::at(PADDING, y, Length::Px(c), Length::Px(TOUCH_CONTROL)),
                    );
                    tree.set_place(meaning, text_at(PADDING, y + TOUCH_CONTROL + 4));
                    y + TOUCH_CONTROL + 4 + LINE + 8
                }
                (
                    Row::Control {
                        label,
                        control,
                        width,
                        indented,
                    },
                    None,
                ) => {
                    let x = if indented { INDENT } else { PADDING };
                    tree.set_place(label, text_at(x, y + TEXT_IN_ROW));
                    tree.set_place(control, from_right(y + CONTROL_IN_ROW, width));
                    y + ROW
                }
                (Row::Control { label, control, .. }, Some(c)) => {
                    tree.set_place(label, text_at(PADDING, y));
                    tree.set_place(
                        control,
                        Place::at(PADDING, y + 24, Length::Px(c), Length::Px(TOUCH_CONTROL)),
                    );
                    y + 24 + TOUCH_CONTROL + 8
                }
            };
        }
        let [navigation, always, reaches] = self.navigation;
        tree.set_place(navigation, text_at(PADDING, y + TEXT_IN_ROW));
        match content {
            None => tree.set_place(always, Place::right(PADDING, y + TEXT_IN_ROW)),
            Some(_) => tree.set_place(always, text_at(PADDING, y + TEXT_IN_ROW + LINE)),
        }
        let below = match content {
            None => y + TEXT_IN_ROW + LINE,
            Some(_) => y + TEXT_IN_ROW + 2 * LINE,
        };
        tree.set_place(
            reaches,
            match content {
                None => text_at(PADDING, below),
                Some(c) => Place::at(PADDING, below, Length::Px(c), Length::Content),
            },
        );
        tree.set_place(self.status, text_at(PADDING, below + LINE));
        below + LINE + 2 * PADDING
    }

    /// Shows `shown` in the controls; a dependent control is enabled while its checkbox is.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, shown: Shown) {
        let Shown {
            display,
            keys,
            reader,
        } = shown;
        for (check, on) in [
            (self.contrast, display.high_contrast),
            (self.motion, display.reduce_motion),
            (self.reader, reader.on),
            (self.zoom, display.zoom),
            (self.sticky, keys.sticky_keys),
            (self.slow, keys.slow_keys),
            (self.bounce, keys.bounce_keys),
            (self.toggle, keys.toggle_keys),
            (self.mouse_keys, keys.mouse_keys),
        ] {
            tree.set_selected(check, usize::from(on));
        }
        let level = ZOOM_LEVELS
            .iter()
            .position(|&level| level == display.zoom_level)
            .unwrap_or(0);
        tree.set_selected(self.level, level);
        tree.set_enabled(self.level, display.zoom);
        let size = POINTER_SIZES
            .iter()
            .position(|&(_, halves)| halves == display.pointer_size_halves)
            .unwrap_or(0);
        tree.set_selected(self.pointer_size, size);
        for (slider, value, on) in [
            (self.delay, keys.slow_keys_ms, keys.slow_keys),
            (self.bounce_delay, keys.bounce_keys_ms, keys.bounce_keys),
            (self.top_speed, keys.mouse_keys_speed, keys.mouse_keys),
        ] {
            tree.set_value(slider, value as i32);
            tree.set_enabled(slider, on);
        }
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
            Response::Toggled(id, on) if id == self.motion => Some(DisplayChange::ReduceMotion(on)),
            Response::Toggled(id, on) if id == self.zoom => Some(DisplayChange::Zoom(on)),
            Response::Chosen(id, choice) if id == self.level => ZOOM_LEVELS
                .get(choice)
                .map(|&level| DisplayChange::ZoomLevel(level)),
            Response::Chosen(id, choice) if id == self.pointer_size => POINTER_SIZES
                .get(choice)
                .map(|&(_, halves)| DisplayChange::PointerSizeHalves(halves)),
            _ => None,
        };
        if let Some(change) = display {
            let unchanged = change.applied_to(shown.display) == shown.display;
            return Reply::Done((!unchanged).then_some(Request::ChangeDisplay(change)));
        }
        if let Response::Toggled(id, on) = response
            && id == self.reader
        {
            let change = ReaderChange::On(on);
            let unchanged = change.applied_to(shown.reader) == shown.reader;
            return Reply::Done((!unchanged).then_some(Request::ChangeReader(change)));
        }
        let keys = shown.keys;
        let change = match response {
            Response::Toggled(id, on) if id == self.sticky => KeysChange::StickyKeys(on),
            Response::Toggled(id, on) if id == self.slow => KeysChange::SlowKeys(on),
            Response::Toggled(id, on) if id == self.bounce => KeysChange::BounceKeys(on),
            Response::Toggled(id, on) if id == self.toggle => KeysChange::ToggleKeys(on),
            Response::Toggled(id, on) if id == self.mouse_keys => KeysChange::MouseKeys(on),
            Response::Adjusted(id, value) if id == self.delay => {
                KeysChange::SlowKeysMs(value as u32)
            }
            Response::Adjusted(id, value) if id == self.bounce_delay => {
                KeysChange::BounceKeysMs(value as u32)
            }
            Response::Adjusted(id, value) if id == self.top_speed => {
                KeysChange::MouseKeysSpeed(value as u32)
            }
            _ => return Reply::NotMine,
        };
        let unchanged = change.applied_to(keys) == keys;
        Reply::Done((!unchanged).then_some(Request::ChangeKeys(change)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_options_follow_the_settings_values() {
        assert_eq!(LEVEL_OPTIONS.len(), ZOOM_LEVELS.len());
        assert_eq!(POINTER_SIZE_OPTIONS.len(), POINTER_SIZES.len());
        assert_eq!(POINTER_SIZES[1], ("1.5", 3));
    }
}
