//! Text input (`zwp_text_input_manager_v3`, D176): a client's text fields tell the compositor
//! when they take text, the text around the cursor, what kind of text they want and where the
//! cursor is, each change double-buffered until `commit`. EggOS has no input method yet, so the
//! compositor sends no text of its own (keys still arrive as keys); what a field tells it makes
//! the focused field part of the window's element tree (D161), so agents read the text around
//! the cursor, except a password's or anything marked sensitive. Each text input is told
//! `enter` and `leave` as the keyboard focus enters and leaves its client's surfaces.

use crate::client::ClientSlot;
use crate::compositor::{Message, new_object, object_index};
use eggos_wayland::protocol::{Interface, display, text_input};
use eggos_wayland::wire::Value;

/// Text inputs per client.
pub const MAX_TEXT_INPUTS: usize = 4;
/// The most bytes of the text around the cursor kept: what an element's value holds.
pub const SURROUNDING_CAPACITY: usize = 240;

/// What a text field said about itself, as of its last `commit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldState {
    pub enabled: bool,
    surrounding: [u8; SURROUNDING_CAPACITY],
    surrounding_length: u16,
    /// The cursor's and the selection's other end's byte offsets in the kept text.
    pub cursor: u32,
    pub anchor: u32,
    pub hint: u32,
    pub purpose: u32,
    /// The cursor's rectangle in the surface: x, y, width, height.
    pub cursor_rectangle: (i32, i32, i32, i32),
}

impl FieldState {
    /// Disabled, with nothing known.
    pub const fn new() -> Self {
        Self {
            enabled: false,
            surrounding: [0; SURROUNDING_CAPACITY],
            surrounding_length: 0,
            cursor: 0,
            anchor: 0,
            hint: 0,
            purpose: 0,
            cursor_rectangle: (0, 0, 0, 0),
        }
    }

    /// The text around the cursor that was kept.
    pub fn surrounding(&self) -> &[u8] {
        &self.surrounding[..usize::from(self.surrounding_length)]
    }

    /// Whether its text may not be shown to anyone else: a password, a PIN, or text the field
    /// hides or calls sensitive.
    pub fn is_concealed(&self) -> bool {
        matches!(
            self.purpose,
            text_input::PURPOSE_PASSWORD | text_input::PURPOSE_PIN
        ) || self.hint & (text_input::HINT_HIDDEN_TEXT | text_input::HINT_SENSITIVE_DATA) != 0
    }

    /// Its kind of text, as an element's name: `text` for ordinary text, else the purpose's
    /// own name (`email`, `password`, ...).
    pub fn kind_name(&self) -> &'static str {
        match self.purpose {
            0 => "text",
            purpose => text_input::PURPOSES
                .get(purpose as usize)
                .copied()
                .unwrap_or("text"),
        }
    }

    fn set_surrounding(&mut self, text: &[u8], cursor: i32, anchor: i32) {
        let length = text.len();
        let cursor = (cursor.max(0) as usize).min(length);
        let anchor = (anchor.max(0) as usize).min(length);
        let (start, end) = excerpt(text, cursor);
        self.surrounding = [0; SURROUNDING_CAPACITY];
        self.surrounding[..end - start].copy_from_slice(&text[start..end]);
        self.surrounding_length = (end - start) as u16;
        self.cursor = (cursor.clamp(start, end) - start) as u32;
        self.anchor = (anchor.clamp(start, end) - start) as u32;
    }
}

impl Default for FieldState {
    fn default() -> Self {
        Self::new()
    }
}

/// The part of `text` kept around byte offset `cursor`: at most [`SURROUNDING_CAPACITY`]
/// bytes, centred on the cursor where the text allows, cut at whole UTF-8 characters.
pub fn excerpt(text: &[u8], cursor: usize) -> (usize, usize) {
    let length = text.len();
    if length <= SURROUNDING_CAPACITY {
        return (0, length);
    }
    let cursor = cursor.min(length);
    let end = cursor
        .saturating_add(SURROUNDING_CAPACITY / 2)
        .max(SURROUNDING_CAPACITY)
        .min(length);
    let mut start = end - SURROUNDING_CAPACITY;
    let mut end = end;
    let continues = |at: usize| text.get(at).is_some_and(|&byte| byte & 0xc0 == 0x80);
    while start < end && continues(start) {
        start += 1;
    }
    while end > start && end < length && continues(end) {
        end -= 1;
    }
    (start, end)
}

/// One `zwp_text_input_v3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextInput {
    pub id: u32,
    pub version: u32,
    /// The surface it was told `enter` for; it is ignored while it has none.
    pub entered: Option<u16>,
    pub pending: FieldState,
    pub current: FieldState,
    /// The `commit` requests it made.
    pub commits: u32,
}

/// `zwp_text_input_manager_v3`'s `get_text_input`: a new text input for the seat, told `enter`
/// at once if its client has the keyboard focus.
pub(crate) fn manager_request(client: &mut ClientSlot, message: &Message) {
    if message.opcode != 1 {
        return;
    }
    let id = message.uint(0);
    if object_index(client, message.uint(1), Interface::Seat).is_none() {
        return;
    }
    let Some(free) = client.text_inputs.iter().position(Option::is_none) else {
        client.protocol_error(id, display::NO_MEMORY, b"too many text inputs");
        return;
    };
    if !new_object(
        client,
        id,
        Interface::TextInput,
        message.version,
        free as u16,
    ) {
        return;
    }
    client.text_inputs[free] = Some(TextInput {
        id,
        version: message.version,
        entered: None,
        pending: FieldState::new(),
        current: FieldState::new(),
        commits: 0,
    });
    let focus = client.keyboard_focus;
    enter(client, free, focus);
}

/// A text input's requests, ignored while it has no `enter`: state for the next `commit`,
/// which applies it.
pub(crate) fn request(client: &mut ClientSlot, message: &Message) {
    let index = usize::from(message.index);
    if message.opcode == 0 {
        if let Some(slot) = client.text_inputs.get_mut(index) {
            *slot = None;
        }
        return;
    }
    let Some(Some(input)) = client.text_inputs.get_mut(index) else {
        return;
    };
    if input.entered.is_none() {
        return;
    }
    let pending = &mut input.pending;
    match message.opcode {
        1 => {
            *pending = FieldState::new();
            pending.enabled = true;
        }
        2 => pending.enabled = false,
        3 => pending.set_surrounding(message.values[0].bytes(), message.int(1), message.int(2)),
        5 => {
            pending.hint = message.uint(0);
            pending.purpose = message.uint(1);
        }
        6 => {
            pending.cursor_rectangle = (
                message.int(0),
                message.int(1),
                message.int(2),
                message.int(3),
            );
        }
        7 => {
            input.current = input.pending;
            input.commits = input.commits.wrapping_add(1);
        }
        _ => {}
    }
}

fn enter(client: &mut ClientSlot, index: usize, focus: Option<u16>) {
    let Some(surface) = focus.and_then(|focus| client.surface(focus).map(|surface| surface.id))
    else {
        return;
    };
    let Some(Some(input)) = client.text_inputs.get_mut(index) else {
        return;
    };
    if input.entered == focus {
        return;
    }
    input.entered = focus;
    let (id, version) = (input.id, input.version);
    client.post(id, version, text_input::ENTER, &[Value::Object(surface)]);
}

/// The keyboard focus moved within `client` to `focus` (`None` when it left the client): each
/// text input entered elsewhere is told `leave` and forgets its state, then each is told
/// `enter` for the new focus.
pub fn focus_moved(client: &mut ClientSlot, focus: Option<u16>) {
    for index in 0..client.text_inputs.len() {
        let Some(input) = client.text_inputs[index] else {
            continue;
        };
        if let Some(left) = input.entered.filter(|&left| Some(left) != focus) {
            let surface = client.surface(left).map(|surface| surface.id);
            if let Some(Some(slot)) = client.text_inputs.get_mut(index) {
                slot.entered = None;
                slot.pending = FieldState::new();
                slot.current = FieldState::new();
            }
            if let Some(surface) = surface {
                client.post(
                    input.id,
                    input.version,
                    text_input::LEAVE,
                    &[Value::Object(surface)],
                );
            }
        }
        enter(client, index, focus);
    }
}

/// Surface `index` is going: the text inputs entered on it forget it without a `leave`, as
/// its object is gone.
pub fn forget_surface(client: &mut ClientSlot, index: u16) {
    for input in client.text_inputs.iter_mut().flatten() {
        if input.entered == Some(index) {
            input.entered = None;
            input.pending = FieldState::new();
            input.current = FieldState::new();
        }
    }
}

/// The enabled text field on a surface whose window is `is_in_window`'s, if there is one:
/// the surface and what the field last committed.
pub fn focused_field(
    client: &ClientSlot,
    is_in_window: impl Fn(u16) -> bool,
) -> Option<(u16, FieldState)> {
    client
        .text_inputs
        .iter()
        .flatten()
        .filter(|input| input.current.enabled)
        .find_map(|input| {
            input
                .entered
                .filter(|&surface| is_in_window(surface))
                .map(|surface| (surface, input.current))
        })
}
