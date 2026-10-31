//! Seeing and operating any window for a command or an agent (D141, docs/windows.md): what a
//! capture of a window's content reads ([`ContentLayout`], `CAPTURE_WINDOW`), and input sent
//! straight to one window's queue ([`Desktop::send_input`], `SEND_INPUT`), never through the
//! shortcuts, the popups or hit-testing, without moving the person's pointer and without the
//! window on screen. A person wins: while they hold a button or for [`PERSON_WINS_MS`] after
//! their last input, injected input is refused ([`WindowError::Busy`]).

use super::protocol::{
    DRAG_STEPS, EventRecord, EventSink, MAX_WHEEL_NOTCHES, WindowError, event, input_kind,
    modifier, query_kind,
};
use super::{Desktop, KERNEL_CLIENT, WindowId, codepoint, colour, key_modifiers};
use crate::display::{Damage, Rect};
use crate::input::keymap::{Modifiers, key_for_ascii};
use crate::input::{button, key};

/// How long after a person's last key, button, wheel notch or pointer motion injected input
/// is refused.
pub const PERSON_WINS_MS: u32 = 500;

/// How far the pointer must move from where a person last moved it, in screen pixels either
/// way, to count as their input again: a resting hand's jitter does not.
pub const PERSON_MOTION_THRESHOLD_PX: u32 = 4;

const MODIFIER_KEYS: [(Modifiers, u16); 4] = [
    (Modifiers::SHIFT, key::LEFT_SHIFT),
    (Modifiers::CTRL, key::LEFT_CTRL),
    (Modifiers::ALT, key::LEFT_ALT),
    (Modifiers::SUPER, key::LEFT_SUPER),
];

const KNOWN_MODIFIERS: u32 = modifier::SHIFT | modifier::CTRL | modifier::ALT | modifier::SUPER;

const DRAG_MOVES: i32 = DRAG_STEPS as i32;

/// One step of input for one window, as `SEND_INPUT` asks for it. Coordinates are pixels of
/// the window's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedInput {
    /// The pointer moves to (`x`, `y`).
    PointerMove { x: u32, y: u32 },
    /// `button` (`BTN_LEFT`, `BTN_RIGHT` or `BTN_MIDDLE`) is pressed and released at (`x`, `y`).
    Click { x: u32, y: u32, button: u16 },
    /// `button` is pressed and released twice at (`x`, `y`) in one step: a double-click.
    DoubleClick { x: u32, y: u32, button: u16 },
    /// `button` is pressed at (`x`, `y`), the pointer moves to (`to_x`, `to_y`) in
    /// [`DRAG_STEPS`] moves, and the button is released there.
    Drag {
        x: u32,
        y: u32,
        to_x: u32,
        to_y: u32,
        button: u16,
    },
    /// The wheel turns by `notches`, up when positive, and sideways by `sideways`, right when
    /// positive, at (`x`, `y`).
    Wheel {
        x: u32,
        y: u32,
        notches: i32,
        sideways: i32,
    },
    /// Key `code` is pressed and released with `modifiers` held.
    Key { code: u16, modifiers: Modifiers },
    /// The character, not a control character, is typed: as the US keymap types it when a key
    /// does, else as a `Key` record of code 0 that carries it, a character with no key of its
    /// own (D185).
    Character(char),
}

impl InjectedInput {
    /// The input `SEND_INPUT`'s fields ask for; [`WindowError::Invalid`] for an unknown kind, a
    /// negative coordinate, a button other than left, right and middle, a key code that is 0
    /// or a button's, modifier bits other than Shift, Ctrl, Alt and Super, no wheel notch on
    /// either axis, or a character that is not printable ASCII. Notches past
    /// [`MAX_WHEEL_NOTCHES`] are clamped.
    pub fn from_request(
        kind: u32,
        x: i32,
        y: i32,
        code: u32,
        value: u32,
    ) -> Result<Self, WindowError> {
        let point = || {
            Ok::<_, WindowError>((
                u32::try_from(x).map_err(|_| WindowError::Invalid)?,
                u32::try_from(y).map_err(|_| WindowError::Invalid)?,
            ))
        };
        let pointer_button = || {
            u16::try_from(code)
                .ok()
                .filter(|&code| matches!(code, button::LEFT | button::RIGHT | button::MIDDLE))
                .ok_or(WindowError::Invalid)
        };
        match kind {
            input_kind::POINTER_MOVE => {
                let (x, y) = point()?;
                Ok(Self::PointerMove { x, y })
            }
            input_kind::CLICK => {
                let (x, y) = point()?;
                Ok(Self::Click {
                    x,
                    y,
                    button: pointer_button()?,
                })
            }
            input_kind::DOUBLE_CLICK => {
                let (x, y) = point()?;
                Ok(Self::DoubleClick {
                    x,
                    y,
                    button: pointer_button()?,
                })
            }
            input_kind::DRAG => {
                let (x, y) = point()?;
                Ok(Self::Drag {
                    x,
                    y,
                    to_x: value & 0xffff,
                    to_y: value >> 16,
                    button: pointer_button()?,
                })
            }
            input_kind::WHEEL => {
                let (x, y) = point()?;
                let notches = (value as i32).clamp(-MAX_WHEEL_NOTCHES, MAX_WHEEL_NOTCHES);
                let sideways = (code as i32).clamp(-MAX_WHEEL_NOTCHES, MAX_WHEEL_NOTCHES);
                if notches == 0 && sideways == 0 {
                    return Err(WindowError::Invalid);
                }
                Ok(Self::Wheel {
                    x,
                    y,
                    notches,
                    sideways,
                })
            }
            input_kind::KEY => {
                let code = u16::try_from(code)
                    .ok()
                    .filter(|&code| code != 0 && code < button::FIRST)
                    .ok_or(WindowError::Invalid)?;
                if value & !KNOWN_MODIFIERS != 0 {
                    return Err(WindowError::Invalid);
                }
                Ok(Self::Key {
                    code,
                    modifiers: modifiers_of(value),
                })
            }
            input_kind::CHARACTER => char::from_u32(code)
                .filter(|character| !character.is_control())
                .map(Self::Character)
                .ok_or(WindowError::Invalid),
            _ => Err(WindowError::Invalid),
        }
    }
}

fn modifiers_of(bits: u32) -> Modifiers {
    [
        (modifier::SHIFT, Modifiers::SHIFT),
        (modifier::CTRL, Modifiers::CTRL),
        (modifier::ALT, Modifiers::ALT),
        (modifier::SUPER, Modifiers::SUPER),
    ]
    .into_iter()
    .filter(|(bit, _)| bits & bit != 0)
    .fold(Modifiers::NONE, |held, (_, modifier)| held.with(modifier))
}

/// Input for the terminal's window, which the kernel hands to the terminal's own handlers once
/// the desktop's lock is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalInput {
    /// Key `code` pressed with `modifiers`: its bytes go to the console.
    Key { code: u16, modifiers: Modifiers },
    /// The character goes to the console, in UTF-8.
    Character(char),
    /// The view scrolls by `notches`, back when positive.
    Scroll { notches: i32 },
}

/// Where [`Desktop::send_input`] put the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injected {
    /// In the window's client's queue.
    Queued,
    /// The window has a dialog open: the key or character went to window `dialog`, the
    /// dialog, as a person's keys would.
    QueuedForDialog { dialog: u32 },
    /// Nowhere: the window has dialog `dialog` open, and a point in the window means nothing
    /// in the dialog.
    DialogOpen { dialog: u32 },
    /// Nowhere yet: it is the terminal's, for the caller to hand over.
    ForTerminal(TerminalInput),
}

impl InjectedInput {
    /// The point in the window's content the input aims at, its end point for a drag; `None`
    /// for keys and characters.
    pub fn point(self) -> Option<(u32, u32)> {
        match self {
            InjectedInput::PointerMove { x, y }
            | InjectedInput::Click { x, y, .. }
            | InjectedInput::DoubleClick { x, y, .. }
            | InjectedInput::Wheel { x, y, .. } => Some((x, y)),
            InjectedInput::Drag { to_x, to_y, .. } => Some((to_x, to_y)),
            InjectedInput::Key { .. } | InjectedInput::Character(_) => None,
        }
    }

    fn records(self) -> usize {
        match self {
            InjectedInput::PointerMove { .. } => 1,
            InjectedInput::Click { .. } => 3,
            InjectedInput::DoubleClick { .. } => 5,
            InjectedInput::Drag { .. } => DRAG_STEPS as usize + 3,
            InjectedInput::Wheel {
                notches, sideways, ..
            } => 1 + usize::from(notches != 0) + usize::from(sideways != 0),
            InjectedInput::Key { modifiers, .. } => 2 + 2 * held_modifier_keys(modifiers),
            InjectedInput::Character(_) => 2,
        }
    }
}

/// What a window's content shows: its size, the part its buffer covers and the colour of the
/// rest, as the desktop draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentLayout {
    /// The content's width in pixels.
    pub width: u32,
    /// The content's height in pixels.
    pub height: u32,
    /// Where the buffer's pixels sit in the content; empty before the window is ready.
    pub buffer: Rect,
    /// The colour of the content outside the buffer.
    pub fill: u32,
}

impl ContentLayout {
    /// Copies content pixels `x..x + out.len()` of row `y` into `out`: the buffer's, which
    /// `buffer_row` copies given their column and row in the buffer, and the fill around them.
    pub fn copy_row(
        &self,
        x: u32,
        y: u32,
        out: &mut [u32],
        buffer_row: &mut dyn FnMut(u32, u32, &mut [u32]),
    ) {
        let in_buffer = y >= self.buffer.y && y < self.buffer.bottom() && self.buffer.width > 0;
        if !in_buffer {
            out.fill(self.fill);
            return;
        }
        let end = x.saturating_add(out.len() as u32);
        let copy_left = x.max(self.buffer.x).min(end);
        let copy_right = end.min(self.buffer.right()).max(copy_left);
        let (left, rest) = out.split_at_mut((copy_left - x) as usize);
        let (copied, right) = rest.split_at_mut((copy_right - copy_left) as usize);
        left.fill(self.fill);
        right.fill(self.fill);
        if !copied.is_empty() {
            buffer_row(copy_left - self.buffer.x, y - self.buffer.y, copied);
        }
    }
}

impl Desktop {
    /// What window `window`'s content shows, for a capture; [`WindowError::NoSuchWindow`]
    /// for no such window, [`WindowError::Unsupported`] for the terminal's, which the kernel
    /// draws.
    pub fn content_layout(&self, window: u32) -> Result<ContentLayout, WindowError> {
        let slot = self.any_window(window)?;
        let window = self.window_at(slot);
        if window.client == KERNEL_CLIENT {
            return Err(WindowError::Unsupported);
        }
        Ok(ContentLayout {
            width: window.content_width,
            height: window.content_height,
            buffer: match window.ready {
                true => window.buffer,
                false => Rect::default(),
            },
            fill: match window.ready {
                true => window.fill,
                false => colour::WINDOW_BACKGROUND,
            },
        })
    }

    /// Whether a person is using the machine: a button is held, a window is dragged or
    /// resized, or their last input was less than [`PERSON_WINS_MS`] ago.
    pub fn person_is_active(&self) -> bool {
        self.buttons != 0
            || self.grab.is_some()
            || self.drag.is_some()
            || self.resize.is_some()
            || self
                .person_input_ms
                .is_some_and(|at| self.now_ms.wrapping_sub(at) < PERSON_WINS_MS)
    }

    pub(super) fn note_person_input(&mut self) {
        self.person_input_ms = Some(self.now_ms);
    }

    pub(super) fn note_person_motion(&mut self, x: u32, y: u32) {
        let (from_x, from_y) = self.person_pointer;
        if x.abs_diff(from_x).max(y.abs_diff(from_y)) > PERSON_MOTION_THRESHOLD_PX {
            self.person_pointer = (x, y);
            self.note_person_input();
        }
    }

    pub(super) fn any_window(&self, window: u32) -> Result<usize, WindowError> {
        let id = WindowId::try_from(window).map_err(|_| WindowError::NoSuchWindow)?;
        self.find(id).ok_or(WindowError::NoSuchWindow)
    }

    /// The client whose app draws window `window`. [`WindowError::NoSuchWindow`] for no such
    /// window, [`WindowError::Unsupported`] for the terminal's, which the kernel draws.
    pub fn owner_of(&self, window: u32) -> Result<u16, WindowError> {
        let slot = self.any_window(window)?;
        match self.window_at(slot).client {
            KERNEL_CLIENT => Err(WindowError::Unsupported),
            client => Ok(client),
        }
    }

    /// Hands window `window`'s app the `Query` record `query` (D161, `GET_TREE` and
    /// `ACT_ON_ELEMENT`): a request for its element tree, or for an action on one element when
    /// the record's `b` is [`query_kind::ACT`]; returns the app's client, which answers it.
    /// [`WindowError::NoSuchWindow`] for no such window, [`WindowError::Unsupported`] for the
    /// terminal's window (the kernel draws it), [`WindowError::Busy`] for an action while
    /// [`Self::person_is_active`], [`WindowError::QueueFull`] when the client's queue has no
    /// room (nothing is sent).
    pub fn ask_owner(
        &mut self,
        window: u32,
        query: EventRecord,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<u16, WindowError> {
        let client = self.owner_of(window)?;
        let slot = self.any_window(window)?;
        if query.b as u32 == query_kind::ACT && self.person_is_active() {
            return Err(WindowError::Busy);
        }
        if events.room(client) == 0 {
            return Err(WindowError::QueueFull);
        }
        self.note_agent_action(damage);
        self.send(
            slot,
            EventRecord {
                kind: event::QUERY,
                ..query
            },
            events,
        );
        Ok(client)
    }

    /// Sends `input` to window `window`, any client's, as one step: straight to its client's
    /// queue, with `FocusIn` before and `FocusOut` after unless it is the focused window, and
    /// `PointerEnter` and `PointerLeave` around pointer input unless the pointer is over it; a
    /// click is a move, a press and a release, a double-click two of those presses and
    /// releases, a drag a press, [`DRAG_STEPS`] moves and a release; the wheel is one record
    /// for each axis it turns; a key is its modifiers' keys down, the key down and up, and the
    /// modifiers up. Focus, hover and the pointer stay as they were. A window with a dialog
    /// open passes keys and characters to the dialog ([`Injected::QueuedForDialog`]) and
    /// refuses pointer input ([`Injected::DialogOpen`], nothing sent), as the desktop treats a
    /// person's. The terminal's keys, characters and wheel come back as
    /// [`Injected::ForTerminal`]. [`WindowError::NoSuchWindow`] for no such window,
    /// [`WindowError::Busy`] while [`Self::person_is_active`], [`WindowError::Invalid`] for a
    /// point outside the content or a character no key types, [`WindowError::Unsupported`] for
    /// pointer input or sideways scrolling in the terminal, [`WindowError::QueueFull`] when the
    /// client's queue lacks room for the whole step (nothing is sent), [`WindowError::NotAllowed`]
    /// for a window that takes only the person's input (D193's grant prompt).
    pub fn send_input(
        &mut self,
        window: u32,
        input: InjectedInput,
        events: &mut dyn EventSink,
    ) -> Result<Injected, WindowError> {
        let slot = self.any_window(window)?;
        if self.window_at(slot).takes_person_input_only() {
            return Err(WindowError::NotAllowed);
        }
        if self.person_is_active() {
            return Err(WindowError::Busy);
        }
        if self.window_at(slot).id == self.terminal_window() {
            return match input {
                InjectedInput::Key { code, modifiers } => {
                    Ok(Injected::ForTerminal(TerminalInput::Key {
                        code,
                        modifiers,
                    }))
                }
                InjectedInput::Character(character) => {
                    Ok(Injected::ForTerminal(TerminalInput::Character(character)))
                }
                InjectedInput::Wheel {
                    notches,
                    sideways: 0,
                    ..
                } if notches != 0 => Ok(Injected::ForTerminal(TerminalInput::Scroll { notches })),
                _ => Err(WindowError::Unsupported),
            };
        }
        match self.open_dialog(slot) {
            Some(dialog) if input.point().is_some() => Ok(Injected::DialogOpen {
                dialog: u32::from(self.window_at(dialog).id),
            }),
            Some(dialog) => {
                self.queue_input(dialog, input, events)?;
                Ok(Injected::QueuedForDialog {
                    dialog: u32::from(self.window_at(dialog).id),
                })
            }
            None => {
                self.queue_input(slot, input, events)?;
                Ok(Injected::Queued)
            }
        }
    }

    fn open_dialog(&self, slot: usize) -> Option<usize> {
        self.stack[..self.stack_length]
            .iter()
            .rev()
            .map(|&dialog| usize::from(dialog))
            .find(|&dialog| self.window_at(dialog).parent == Some(slot as u8))
    }

    fn queue_input(
        &mut self,
        slot: usize,
        input: InjectedInput,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let target = *self.window_at(slot);
        if target.takes_person_input_only() {
            return Err(WindowError::NotAllowed);
        }
        if target.client == KERNEL_CLIENT {
            return Err(WindowError::Unsupported);
        }
        let input = match input {
            InjectedInput::Character(character) => {
                u8::try_from(character).ok().and_then(key_for_ascii).map_or(
                    input,
                    |(code, modifiers)| InjectedInput::Key { code, modifiers },
                )
            }
            other => other,
        };
        let start = match input {
            InjectedInput::Drag { x, y, .. } => Some((x, y)),
            other => other.point(),
        };
        let outside = |point: Option<(u32, u32)>| {
            point.is_some_and(|(x, y)| x >= target.content_width || y >= target.content_height)
        };
        if outside(start) || outside(input.point()) {
            return Err(WindowError::Invalid);
        }
        let focus = self.focus != Some(slot);
        let hover = start.is_some() && self.hover != Some(slot);
        let records = 2 * usize::from(focus) + 2 * usize::from(hover) + input.records();
        if events.room(target.client) < records {
            return Err(WindowError::QueueFull);
        }
        let send = |kind: u16, a: i32, b: i32, c: u32, d: u32, events: &mut dyn EventSink| {
            events.send(target.client, EventRecord::new(kind, target.id, a, b, c, d));
        };
        if focus {
            send(event::FOCUS_IN, 0, 0, 0, 0, events);
        }
        if let Some((x, y)) = start {
            let (x, y) = (x as i32, y as i32);
            if hover {
                send(event::POINTER_ENTER, x, y, 0, 0, events);
            }
            send(event::POINTER_MOVE, x, y, 0, 0, events);
            match input {
                InjectedInput::Click { button, .. } => {
                    send(event::BUTTON, x, y, u32::from(button), 1, events);
                    send(event::BUTTON, x, y, u32::from(button), 0, events);
                }
                InjectedInput::DoubleClick { button, .. } => {
                    for _ in 0..2 {
                        send(event::BUTTON, x, y, u32::from(button), 1, events);
                        send(event::BUTTON, x, y, u32::from(button), 0, events);
                    }
                }
                InjectedInput::Drag {
                    to_x, to_y, button, ..
                } => {
                    send(event::BUTTON, x, y, u32::from(button), 1, events);
                    let (to_x, to_y) = (to_x as i32, to_y as i32);
                    let held = held_button_bit(button);
                    for step in 1..=DRAG_MOVES {
                        let along = |from: i32, to: i32| from + (to - from) * step / DRAG_MOVES;
                        send(
                            event::POINTER_MOVE,
                            along(x, to_x),
                            along(y, to_y),
                            held,
                            0,
                            events,
                        );
                    }
                    send(event::BUTTON, to_x, to_y, u32::from(button), 0, events);
                }
                InjectedInput::Wheel {
                    notches, sideways, ..
                } => {
                    if notches != 0 {
                        send(event::WHEEL, x, y, 0, notches as u32, events);
                    }
                    if sideways != 0 {
                        send(event::WHEEL, x, y, sideways as u32, 0, events);
                    }
                }
                _ => {}
            }
            if hover {
                send(event::POINTER_LEAVE, 0, 0, 0, 0, events);
            }
        }
        if let InjectedInput::Key { code, modifiers } = input {
            let key_record =
                |code: u16, pressed: bool, held: Modifiers, events: &mut dyn EventSink| {
                    send(
                        event::KEY,
                        i32::from(code),
                        i32::from(pressed),
                        key_modifiers(held),
                        codepoint(code, held),
                        events,
                    );
                };
            let mut held = Modifiers::NONE;
            for (modifier, modifier_key) in MODIFIER_KEYS {
                if modifiers.contains(modifier) && code != modifier_key {
                    held = held.with(modifier);
                    key_record(modifier_key, true, held, events);
                }
            }
            key_record(code, true, modifiers, events);
            key_record(code, false, modifiers, events);
            for (modifier, modifier_key) in MODIFIER_KEYS.into_iter().rev() {
                if modifiers.contains(modifier) && code != modifier_key {
                    held = Modifiers::from_bits(held.bits() & !modifier.bits());
                    key_record(modifier_key, false, held, events);
                }
            }
        }
        if let InjectedInput::Character(character) = input {
            send(event::KEY, 0, 1, 0, u32::from(character), events);
            send(event::KEY, 0, 0, 0, u32::from(character), events);
        }
        if focus {
            send(event::FOCUS_OUT, 0, 0, 0, 0, events);
        }
        Ok(())
    }
}

fn held_button_bit(code: u16) -> u32 {
    match code {
        button::LEFT => 1,
        button::RIGHT => 2,
        _ => 4,
    }
}

fn held_modifier_keys(modifiers: Modifiers) -> usize {
    MODIFIER_KEYS
        .iter()
        .filter(|(modifier, _)| modifiers.contains(*modifier))
        .count()
}
