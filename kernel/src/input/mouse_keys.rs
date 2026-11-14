//! Mouse keys (D602, docs/ux/accessibility-v0.4.md section 3.4): the numeric keypad moves the
//! pointer and clicks, whatever Num Lock says. A held direction moves 1 pixel at once, then
//! every [`MOUSE_KEYS_TICK_MS`]; the step grows by a pixel every 4 ticks up to a fiftieth of the
//! top speed. Shift held moves 1 pixel a tick; Ctrl held moves 4 times the step, at most 40.

use eggos_drivers::input::key;

/// How often a held direction moves the pointer, in milliseconds.
pub const MOUSE_KEYS_TICK_MS: u64 = 20;
const GROWTH_TICKS: u32 = 4;
const TICKS_A_SECOND: u32 = 50;
const CTRL_FACTOR: i32 = 4;
const CTRL_MOST: i32 = 40;

const DIRECTIONS: [(u16, i32, i32); 8] = [
    (key::KEYPAD_7, -1, -1),
    (key::KEYPAD_8, 0, -1),
    (key::KEYPAD_9, 1, -1),
    (key::KEYPAD_4, -1, 0),
    (key::KEYPAD_6, 1, 0),
    (key::KEYPAD_1, -1, 1),
    (key::KEYPAD_2, 0, 1),
    (key::KEYPAD_3, 1, 1),
];

/// The pointer buttons the keypad selects, in the input layer's button order (left, right,
/// middle), with their keys.
const BUTTON_KEYS: [(u16, u8); 3] = [
    (key::KEYPAD_SLASH, 0),
    (key::KEYPAD_MINUS, 1),
    (key::KEYPAD_ASTERISK, 2),
];

/// What a keypad key asks of the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseKeysAction {
    Nothing,
    /// Moves by this many pixels.
    Move(i32, i32),
    /// Presses and releases the selected button.
    Click,
    /// Clicks it twice.
    DoubleClick,
    /// Presses it and keeps it held.
    Press,
    /// Releases it.
    Release,
}

/// Whether `code` is one of the keypad's keys mouse keys take.
pub(super) fn is_mouse_key(code: u16) -> bool {
    DIRECTIONS.iter().any(|&(direction, _, _)| direction == code)
        || BUTTON_KEYS.iter().any(|&(button, _)| button == code)
        || matches!(
            code,
            key::KEYPAD_5 | key::KEYPAD_PLUS | key::KEYPAD_0 | key::KEYPAD_DOT
        )
}

/// The directions held, the button selected and whether it is held, and the motion's ticks.
#[derive(Debug, Clone, Copy)]
pub(super) struct MouseKeys {
    directions: u8,
    button: u8,
    holding: bool,
    ticks: u32,
    next: Option<u64>,
}

impl MouseKeys {
    pub(super) const fn new() -> Self {
        Self {
            directions: 0,
            button: 0,
            holding: false,
            ticks: 0,
            next: None,
        }
    }

    /// The button selected (0 left, 1 right, 2 middle) and whether it is held.
    pub(super) fn button(&self) -> (u8, bool) {
        (self.button, self.holding)
    }

    /// When the held directions move next, while any is held.
    pub(super) fn next(&self) -> Option<u64> {
        self.next
    }

    fn delta(&self) -> (i32, i32) {
        DIRECTIONS
            .iter()
            .enumerate()
            .filter(|&(index, _)| self.directions & (1 << index) != 0)
            .fold((0, 0), |(x, y), (_, &(_, dx, dy))| (x + dx, y + dy))
    }

    fn step(&self, shift: bool, ctrl: bool, top_speed: u32) -> i32 {
        if shift {
            return 1;
        }
        let most = (top_speed / TICKS_A_SECOND).max(1);
        let step = (1 + self.ticks.saturating_sub(1) / GROWTH_TICKS).min(most) as i32;
        match ctrl {
            true => (step * CTRL_FACTOR).min(CTRL_MOST),
            false => step,
        }
    }

    /// Takes key `code`'s change `value` at `now`, a tick being `tick` clock ticks long.
    pub(super) fn key(
        &mut self,
        code: u16,
        value: i32,
        now: u64,
        tick: u64,
        (shift, ctrl, top_speed): (bool, bool, u32),
    ) -> MouseKeysAction {
        if let Some(index) = DIRECTIONS.iter().position(|&(direction, _, _)| direction == code) {
            let bit = 1 << index;
            return match value {
                1 => {
                    self.directions |= bit;
                    if self.next.is_some() {
                        return MouseKeysAction::Nothing;
                    }
                    self.ticks = 1;
                    self.next = Some(now + tick);
                    let (dx, dy) = self.delta();
                    let step = self.step(shift, ctrl, top_speed);
                    MouseKeysAction::Move(dx * step, dy * step)
                }
                0 => {
                    self.directions &= !bit;
                    if self.directions == 0 {
                        self.next = None;
                        self.ticks = 0;
                    }
                    MouseKeysAction::Nothing
                }
                _ => MouseKeysAction::Nothing,
            };
        }
        if value != 1 {
            return MouseKeysAction::Nothing;
        }
        if let Some(&(_, button)) = BUTTON_KEYS.iter().find(|&&(key, _)| key == code) {
            if !self.holding {
                self.button = button;
            }
            return MouseKeysAction::Nothing;
        }
        match code {
            key::KEYPAD_5 if !self.holding => MouseKeysAction::Click,
            key::KEYPAD_PLUS if !self.holding => MouseKeysAction::DoubleClick,
            key::KEYPAD_0 if !self.holding => {
                self.holding = true;
                MouseKeysAction::Press
            }
            key::KEYPAD_DOT if self.holding => {
                self.holding = false;
                MouseKeysAction::Release
            }
            _ => MouseKeysAction::Nothing,
        }
    }

    /// The move due at `now`, if one is: the held directions' at the step their ticks reached.
    pub(super) fn due(
        &mut self,
        now: u64,
        tick: u64,
        (shift, ctrl, top_speed): (bool, bool, u32),
    ) -> Option<(i32, i32)> {
        let next = self.next?;
        if now < next {
            return None;
        }
        self.ticks = self.ticks.saturating_add(1);
        self.next = Some(now.max(next) + tick);
        let (dx, dy) = self.delta();
        let step = self.step(shift, ctrl, top_speed);
        Some((dx * step, dy * step))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: (bool, bool, u32) = (false, false, 500);

    #[test]
    fn a_held_direction_moves_one_pixel_then_grows_every_four_ticks_to_its_top() {
        let mut keys = MouseKeys::new();
        assert_eq!(
            keys.key(key::KEYPAD_6, 1, 0, 20, PLAIN),
            MouseKeysAction::Move(1, 0)
        );
        let steps: alloc::vec::Vec<i32> = (1..=40)
            .filter_map(|tick| keys.due(tick * 20, 20, PLAIN).map(|(dx, _)| dx))
            .collect();
        assert_eq!(&steps[..8], &[1, 1, 1, 2, 2, 2, 2, 3]);
        assert_eq!(*steps.last().unwrap(), 10);
        assert_eq!(keys.key(key::KEYPAD_6, 0, 820, 20, PLAIN), MouseKeysAction::Nothing);
        assert_eq!(keys.next(), None);
    }

    #[test]
    fn shift_moves_a_pixel_and_ctrl_four_times_at_most_forty() {
        let mut keys = MouseKeys::new();
        assert_eq!(
            keys.key(key::KEYPAD_3, 1, 0, 20, (false, true, 1000)),
            MouseKeysAction::Move(4, 4)
        );
        keys.ticks = 100;
        assert_eq!(keys.step(true, false, 1000), 1);
        assert_eq!(keys.step(false, true, 1000), 40);
    }

    #[test]
    fn zero_holds_the_selected_button_and_the_dot_lets_it_go() {
        let mut keys = MouseKeys::new();
        keys.key(key::KEYPAD_MINUS, 1, 0, 20, PLAIN);
        assert_eq!(keys.button(), (1, false));
        assert_eq!(keys.key(key::KEYPAD_0, 1, 0, 20, PLAIN), MouseKeysAction::Press);
        assert_eq!(keys.button(), (1, true));
        assert_eq!(keys.key(key::KEYPAD_5, 1, 0, 20, PLAIN), MouseKeysAction::Nothing);
        assert_eq!(keys.key(key::KEYPAD_DOT, 1, 0, 20, PLAIN), MouseKeysAction::Release);
        assert!(is_mouse_key(key::KEYPAD_PLUS));
        assert!(!is_mouse_key(key::ENTER));
    }
}
