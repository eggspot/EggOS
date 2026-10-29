//! Seeing and operating any window for a command or an agent (D141, docs/windows.md): what a
//! capture of a window's content reads ([`ContentLayout`], `CAPTURE_WINDOW`), and input sent
//! straight to one window's queue ([`Desktop::send_input`], `SEND_INPUT`), never through the
//! shortcuts, the popups or hit-testing, without moving the person's pointer and without the
//! window on screen. A person wins: while they hold a button or for [`PERSON_WINS_MS`] after
//! their last input, injected input is refused ([`WindowError::Busy`]).

use super::protocol::{
    EventRecord, EventSink, MAX_WHEEL_NOTCHES, WindowError, event, input_kind, modifier,
};
use super::{Desktop, KERNEL_CLIENT, WindowId, codepoint, colour, key_modifiers};
use crate::display::Rect;
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

/// One step of input for one window, as `SEND_INPUT` asks for it. Coordinates are pixels of
/// the window's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedInput {
    /// The pointer moves to (`x`, `y`).
    PointerMove { x: u32, y: u32 },
    /// `button` (`BTN_LEFT`, `BTN_RIGHT` or `BTN_MIDDLE`) is pressed and released at (`x`, `y`).
    Click { x: u32, y: u32, button: u16 },
    /// The wheel turns by `notches`, up when positive, at (`x`, `y`).
    Wheel { x: u32, y: u32, notches: i32 },
    /// Key `code` is pressed and released with `modifiers` held.
    Key { code: u16, modifiers: Modifiers },
    /// The printable ASCII character is typed as the US keymap types it.
    Character(u8),
}

impl InjectedInput {
    /// The input `SEND_INPUT`'s fields ask for; [`WindowError::Invalid`] for an unknown kind, a
    /// negative coordinate, a button other than left, right and middle, a key code that is 0
    /// or a button's, modifier bits other than Shift, Ctrl, Alt and Super, no wheel notch, or a
    /// character that is not printable ASCII. Notches past [`MAX_WHEEL_NOTCHES`] are clamped.
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
        match kind {
            input_kind::POINTER_MOVE => {
                let (x, y) = point()?;
                Ok(Self::PointerMove { x, y })
            }
            input_kind::CLICK => {
                let (x, y) = point()?;
                let button = u16::try_from(code)
                    .ok()
                    .filter(|&code| matches!(code, button::LEFT | button::RIGHT | button::MIDDLE))
                    .ok_or(WindowError::Invalid)?;
                Ok(Self::Click { x, y, button })
            }
            input_kind::WHEEL => {
                let (x, y) = point()?;
                let notches = (value as i32).clamp(-MAX_WHEEL_NOTCHES, MAX_WHEEL_NOTCHES);
                if notches == 0 {
                    return Err(WindowError::Invalid);
                }
                Ok(Self::Wheel { x, y, notches })
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
            input_kind::CHARACTER => u8::try_from(code)
                .ok()
                .filter(|&byte| key_for_ascii(byte).is_some() && (b' '..=b'~').contains(&byte))
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
    /// The character goes to the console.
    Character(u8),
    /// The view scrolls by `notches`, back when positive.
    Scroll { notches: i32 },
}

/// Where [`Desktop::send_input`] put the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injected {
    /// In the window's client's queue.
    Queued,
    /// Nowhere yet: it is the terminal's, for the caller to hand over.
    ForTerminal(TerminalInput),
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

    fn any_window(&self, window: u32) -> Result<usize, WindowError> {
        let id = WindowId::try_from(window).map_err(|_| WindowError::NoSuchWindow)?;
        self.find(id).ok_or(WindowError::NoSuchWindow)
    }

    /// Sends `input` to window `window`, any client's, as one step: straight to its client's
    /// queue, with `FocusIn` before and `FocusOut` after unless it is the focused window, and
    /// `PointerEnter` and `PointerLeave` around pointer input unless the pointer is over it; a
    /// click is a move, a press and a release; a key is its modifiers' keys down, the key down
    /// and up, and the modifiers up. Focus, hover and the pointer stay as they were. The
    /// terminal's keys, characters and wheel come back as [`Injected::ForTerminal`].
    /// [`WindowError::NoSuchWindow`] for no such window, [`WindowError::Busy`] while
    /// [`Self::person_is_active`], [`WindowError::Invalid`] for a point outside the content or
    /// a character no key types, [`WindowError::Unsupported`] for pointer input to the
    /// terminal, [`WindowError::QueueFull`] when the client's queue lacks room for the whole
    /// step (nothing is sent).
    pub fn send_input(
        &mut self,
        window: u32,
        input: InjectedInput,
        events: &mut dyn EventSink,
    ) -> Result<Injected, WindowError> {
        let slot = self.any_window(window)?;
        if self.person_is_active() {
            return Err(WindowError::Busy);
        }
        let target = *self.window_at(slot);
        if target.client == KERNEL_CLIENT {
            return match input {
                InjectedInput::Key { code, modifiers } => {
                    Ok(Injected::ForTerminal(TerminalInput::Key {
                        code,
                        modifiers,
                    }))
                }
                InjectedInput::Character(byte) => {
                    Ok(Injected::ForTerminal(TerminalInput::Character(byte)))
                }
                InjectedInput::Wheel { notches, .. } => {
                    Ok(Injected::ForTerminal(TerminalInput::Scroll { notches }))
                }
                InjectedInput::PointerMove { .. } | InjectedInput::Click { .. } => {
                    Err(WindowError::Unsupported)
                }
            };
        }
        let input = match input {
            InjectedInput::Character(byte) => {
                let (code, modifiers) = key_for_ascii(byte).ok_or(WindowError::Invalid)?;
                InjectedInput::Key { code, modifiers }
            }
            other => other,
        };
        let point = match input {
            InjectedInput::PointerMove { x, y }
            | InjectedInput::Click { x, y, .. }
            | InjectedInput::Wheel { x, y, .. } => Some((x, y)),
            InjectedInput::Key { .. } | InjectedInput::Character(_) => None,
        };
        if let Some((x, y)) = point
            && (x >= target.content_width || y >= target.content_height)
        {
            return Err(WindowError::Invalid);
        }
        let focus = self.focus != Some(slot);
        let hover = point.is_some() && self.hover != Some(slot);
        let records = 2 * usize::from(focus)
            + 2 * usize::from(hover)
            + match input {
                InjectedInput::PointerMove { .. } => 1,
                InjectedInput::Click { .. } => 3,
                InjectedInput::Wheel { .. } => 2,
                InjectedInput::Key { modifiers, .. } => 2 + 2 * held_modifier_keys(modifiers),
                InjectedInput::Character(_) => 0,
            };
        if events.room(target.client) < records {
            return Err(WindowError::QueueFull);
        }
        let send = |kind: u16, a: i32, b: i32, c: u32, d: u32, events: &mut dyn EventSink| {
            events.send(target.client, EventRecord::new(kind, target.id, a, b, c, d));
        };
        if focus {
            send(event::FOCUS_IN, 0, 0, 0, 0, events);
        }
        if let Some((x, y)) = point {
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
                InjectedInput::Wheel { notches, .. } => {
                    send(event::WHEEL, x, y, 0, notches as u32, events);
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
        if focus {
            send(event::FOCUS_OUT, 0, 0, 0, 0, events);
        }
        Ok(Injected::Queued)
    }
}

fn held_modifier_keys(modifiers: Modifiers) -> usize {
    MODIFIER_KEYS
        .iter()
        .filter(|(modifier, _)| modifiers.contains(*modifier))
        .count()
}
