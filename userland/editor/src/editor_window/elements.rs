//! The Text Editor's window as an element tree for an agent (D161, D162): the text as one text
//! field over the text view (the gutter left out), named by the document's name, its text the
//! value (cut to an element's 255 bytes).
//! `set-text` replaces the whole document, one step to undo, and `focus` gives the text the
//! keyboard, as a click in it would.

use super::{EditorWindow, Focus};
use eggos_toolkit::elements::{Action, Bounds, Element, Outcome, Role, TreeWriter, flags};
use eggos_toolkit::tree::ROOT_ELEMENT;

/// The text field: the document's text.
pub const TEXT_ELEMENT: u32 = 10;

impl EditorWindow<'_> {
    /// Writes the text field below [`ROOT_ELEMENT`].
    pub fn describe(&self, tree: &mut TreeWriter<'_>) {
        let view = self.text_view();
        let focused = match self.window_focused && self.focus == Focus::Text {
            true => flags::FOCUSED,
            false => 0,
        };
        let disabled = match self.panel.is_some() {
            true => flags::DISABLED,
            false => 0,
        };
        let text = Element::new(
            TEXT_ELEMENT,
            ROOT_ELEMENT,
            Role::TextField,
            Bounds::new(view.x, view.y, view.width, view.height),
        )
        .named(self.name().as_bytes())
        .valued(self.editor.document().bytes())
        .flagged(flags::FOCUSABLE | focused | disabled);
        tree.push(&text);
    }

    /// Does `action` to `element` for an agent, with `text` for [`Action::SetText`].
    pub fn act(&mut self, element: u32, action: Action, text: &str) -> Outcome {
        if element != TEXT_ELEMENT {
            return Outcome::NoSuchElement;
        }
        if self.panel.is_some() {
            return Outcome::Disabled;
        }
        match action {
            Action::Focus => {
                self.menu = None;
                self.focus = Focus::Text;
                self.restart_caret();
                self.damage_all();
                Outcome::Done
            }
            Action::SetText => {
                self.menu = None;
                self.focus = Focus::Text;
                let result = self.editor.replace_all(text.as_bytes());
                self.edited(result);
                Outcome::Done
            }
            Action::Invoke | Action::Select => Outcome::NotSupported,
        }
    }
}
