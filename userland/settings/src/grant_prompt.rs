//! The grant prompt (D193 slice 41c3, docs/ux/agents-v0.3.md section 5): EggOS's own window
//! that asks the person whether an agent may make a call, started as `settings
//! --grant-prompt` by the MCP server. It shows who asks, the tool and its arguments, counts
//! down to a denial, and answers one word ([`PromptAnswer`]). Its buttons are person-only and
//! its window refuses injected input, so no agent can answer it.

use crate::agents_pane::shortened;
use alloc::string::String;
use core::fmt::Write;
use eggos_mcp::grant_prompt::{PromptAnswer, QuestionRead, offers_always};
use eggos_mcp::grants::{AgentId, Grant, GrantStore, Permission, StoreState};
use eggos_mcp::json::{Object, Str, Value};
use eggos_toolkit::colour;
use eggos_toolkit::input::{Input, key};
use eggos_toolkit::text::width;
use eggos_toolkit::tree::{Response, WidgetId, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};

/// The window's title.
pub const TITLE: &str = "An agent asks";
/// The content's width.
pub const WIDTH: u32 = 480;
/// The content's height.
pub const HEIGHT: u32 = 200;
/// The most widgets the prompt holds.
pub const CAPACITY: usize = 16;
/// How many argument lines the prompt shows.
pub const ARGUMENT_LINES: usize = 3;

const LEFT: i32 = 16;
const RIGHT: i32 = 464;
const NAME_TOP: i32 = 16;
const TOOL_TOP: i32 = 40;
const ARGUMENT_TOPS: [i32; ARGUMENT_LINES] = [68, 88, 108];
const COUNTDOWN_TOP: i32 = 132;
const BUTTON_TOP: i32 = 156;
const BUTTON_HEIGHT: i32 = 28;
const ALWAYS_AT: (i32, i32) = (36, 96);
const SESSION_AT: (i32, i32) = (140, 116);
const ONCE_AT: (i32, i32) = (264, 96);
const DENY_AT: (i32, i32) = (368, 96);
const SECOND_MS: u32 = 1000;

/// The prompt's widgets and what the person answered.
pub struct GrantPrompt {
    tree: WidgetTree<CAPACITY>,
    countdown: WidgetId,
    buttons: [(WidgetId, PromptAnswer); 4],
    started_ms: u32,
    timeout_s: u64,
    shown_left_s: u64,
    answer: Option<PromptAnswer>,
}

impl GrantPrompt {
    /// The prompt for `question`, shown at `now_ms`, with `Deny` focused.
    pub fn new(question: &QuestionRead<'_>, now_ms: u32) -> Self {
        let mut tree = WidgetTree::new();
        let name = shown_text(question.name);
        let id = shown_text(question.agent);
        label(&mut tree, LEFT, NAME_TOP, &name, colour::TEXT);
        let id_line = shortened(&id_text(&id));
        let id_label = label(
            &mut tree,
            RIGHT - width(id_line.as_str()),
            NAME_TOP,
            id_line.as_str(),
            colour::TEXT_DIM,
        );
        tree.set_shown(id_label, name != id);
        let mut tool = shown_text(question.tool);
        tool.push_str(" \u{b7} ");
        tool.push_str(&shown_text(question.title));
        label(&mut tree, LEFT, TOOL_TOP, &tool, colour::TEXT);
        let lines = argument_lines(question.arguments);
        for (top, line) in ARGUMENT_TOPS.into_iter().zip(&lines) {
            let id = label(&mut tree, LEFT, top, line, colour::TEXT);
            tree.set_shown(id, !line.is_empty());
        }
        let countdown = label(
            &mut tree,
            LEFT,
            COUNTDOWN_TOP,
            &countdown_text(question.timeout_s),
            colour::TEXT_DIM,
        );
        let buttons = [
            (
                button(&mut tree, ALWAYS_AT, "Always", false),
                PromptAnswer::Always,
            ),
            (
                button(&mut tree, SESSION_AT, "This session", false),
                PromptAnswer::Session,
            ),
            (
                button(&mut tree, ONCE_AT, "Once", false),
                PromptAnswer::Once,
            ),
            (button(&mut tree, DENY_AT, "Deny", true), PromptAnswer::Deny),
        ];
        for (id, _) in buttons {
            tree.set_person_only(id, true);
        }
        let [(always, _), _, _, (deny, _)] = buttons;
        tree.set_shown(always, offers_always(question.class, question.destructive));
        tree.set_default_button(deny);
        tree.set_focus(Some(deny), now_ms);
        Self {
            tree,
            countdown,
            buttons,
            started_ms: now_ms,
            timeout_s: question.timeout_s,
            shown_left_s: question.timeout_s,
            answer: None,
        }
    }

    /// The widgets.
    pub fn tree(&mut self) -> &mut WidgetTree<CAPACITY> {
        &mut self.tree
    }

    /// The buttons: `Always`, `This session`, `Once`, `Deny`.
    pub fn button_ids(&self) -> [WidgetId; 4] {
        self.buttons.map(|(id, _)| id)
    }

    /// The countdown line.
    pub fn countdown_id(&self) -> WidgetId {
        self.countdown
    }

    /// What the person answered, once they did or the time ran out.
    pub fn answer(&self) -> Option<PromptAnswer> {
        self.answer
    }

    /// Acts on `input` at `now_ms`: Enter and Esc deny from anywhere, Tab and Space go to the
    /// widgets, a pressed button answers.
    pub fn handle(&mut self, input: Input, now_ms: u32) {
        if self.answer.is_some() {
            return;
        }
        if let Input::Key {
            code: key::ENTER | key::ESC,
            pressed: true,
            ..
        } = input
        {
            self.answer = Some(PromptAnswer::Deny);
            return;
        }
        let response = self.tree.handle(input, now_ms);
        self.respond(response);
    }

    /// Acts on what an agent's action on the widgets did (D161): the buttons are person-only,
    /// so nothing an agent does answers.
    pub fn respond(&mut self, response: Response) {
        if let Response::Activated(pressed) = response {
            self.answer = self
                .buttons
                .iter()
                .find(|(id, _)| *id == pressed)
                .map(|&(_, answer)| answer)
                .or(self.answer);
        }
    }

    /// The person closed the window: a denial.
    pub fn close_requested(&mut self) {
        if self.answer.is_none() {
            self.answer = Some(PromptAnswer::Deny);
        }
    }

    /// When the countdown next changes.
    pub fn deadline(&self, now_ms: u32) -> u32 {
        let elapsed = now_ms.wrapping_sub(self.started_ms);
        now_ms.wrapping_add(SECOND_MS - elapsed % SECOND_MS)
    }

    /// Counts down at `now_ms`, damaging the countdown line only when it changes; the time
    /// running out answers [`PromptAnswer::Timeout`].
    pub fn tick(&mut self, now_ms: u32) {
        let elapsed_s = u64::from(now_ms.wrapping_sub(self.started_ms) / SECOND_MS);
        let left_s = self.timeout_s.saturating_sub(elapsed_s);
        if left_s != self.shown_left_s {
            self.shown_left_s = left_s;
            self.tree.set_text(self.countdown, &countdown_text(left_s));
        }
        if left_s == 0 && self.answer.is_none() {
            self.answer = Some(PromptAnswer::Timeout);
        }
    }
}

/// The countdown line: `Denied in 1:30 if you do nothing.`
pub fn countdown_text(left_s: u64) -> String {
    let mut text = String::new();
    let _ = write!(
        text,
        "Denied in {}:{:02} if you do nothing.",
        left_s / 60,
        left_s % 60
    );
    text
}

/// The arguments as `key: value` lines, the server's order (the path first), at most
/// [`ARGUMENT_LINES`]; unused lines are empty.
pub fn argument_lines(arguments: Object<'_>) -> [String; ARGUMENT_LINES] {
    let mut lines: [String; ARGUMENT_LINES] = Default::default();
    for (line, (key, value)) in lines.iter_mut().zip(arguments.members()) {
        line.push_str(&shown_text(key));
        line.push_str(": ");
        match value {
            Value::Str(text) => line.push_str(&shown_text(text)),
            other => line.push_str(&String::from_utf8_lossy(other.raw())),
        }
        *line = String::from(shortened(line).as_str());
    }
    lines
}

/// The store after `Always`: `agent`'s `class` set to `allow` with no end, as `agent grant`
/// does; `None` when the store cannot take it (damaged, or no entry for the agent), so the
/// answer becomes `once`.
pub fn allowed_always(state: StoreState, agent: &AgentId, class: Grant) -> Option<GrantStore> {
    let StoreState::Loaded(mut store) = state else {
        return None;
    };
    store.get_mut(agent)?.set(class, Permission::Allow, None);
    Some(store)
}

fn shown_text(text: Str<'_>) -> String {
    text.chars()
        .map(|character| match character.is_control() {
            true => '?',
            false => character,
        })
        .collect()
}

fn id_text(id: &str) -> String {
    let mut text = String::from("id ");
    text.push_str(id);
    text
}

fn label(tree: &mut WidgetTree<CAPACITY>, x: i32, y: i32, words: &str, ink: u32) -> WidgetId {
    tree.add(
        None,
        Place::at(x, y, Length::Px(RIGHT - x.min(RIGHT)), Length::Content),
        Kind::label(shortened(words).as_str(), ink),
    )
    .expect("the grant prompt fits its tree")
}

fn button(
    tree: &mut WidgetTree<CAPACITY>,
    (x, width): (i32, i32),
    text: &str,
    primary: bool,
) -> WidgetId {
    tree.add(
        None,
        Place::at(x, BUTTON_TOP, Length::Px(width), Length::Px(BUTTON_HEIGHT)),
        Kind::button(text, primary),
    )
    .expect("the grant prompt fits its tree")
}

#[cfg(test)]
mod tests;
