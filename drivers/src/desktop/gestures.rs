//! Touch gestures (D604): the first finger is the pointer and its left button (D296); a second
//! finger makes two fingers, which scroll the window under them as they move together or pinch
//! as they spread or close; a third makes three, which swipe between desktops. Until the
//! fingers have moved past [`SLOP`] nothing is decided, so a press by the first finger works
//! as before; once a gesture is decided, the window that took the press gets `PRESS_CANCELLED`
//! and the first finger's release clicks nothing. Every finger beyond the first also reaches
//! the window it landed on as `TOUCH` records, so an app can follow it itself.

use super::protocol::{EventRecord, EventSink, event};
use super::{Action, Desktop};
use crate::display::Damage;

/// How far, in pixels, the fingers' middle or their spread moves before a gesture is decided.
pub const SLOP: i32 = 16;
/// How far, in pixels, two fingers move together for one wheel notch.
pub const SCROLL_STEP: i32 = 24;
/// How far, in pixels, three fingers move sideways to show the next or previous desktop.
pub const SWIPE_DISTANCE: i32 = 120;
const PINCH_UNIT: i64 = 1000;
const LEFT_BUTTON: u8 = 1;
const FINGERS: usize = 3;

/// What the fingers are doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Kind {
    /// Fewer than two fingers, or a mouse.
    #[default]
    None,
    /// Two or three fingers that have not moved past [`SLOP`] yet.
    Undecided,
    /// Two fingers moving together: the wheel.
    Scroll,
    /// Two fingers spreading or closing.
    Pinch,
    /// Three fingers: one desktop switch at most.
    Swipe { switched: bool },
}

/// The gesture under way.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Gesture {
    kind: Kind,
    fingers: [Option<(i32, i32)>; FINGERS],
    start_middle: (i32, i32),
    start_spread: i32,
    scrolled: (i32, i32),
    /// The window each finger beyond the first landed on, which its `TOUCH` records go to.
    touched: [Option<usize>; FINGERS],
    /// The first finger's release is the gesture's: it clicks nothing.
    swallow_release: bool,
}

fn middle(fingers: &[Option<(i32, i32)>; FINGERS]) -> Option<(i32, i32)> {
    let down = fingers.iter().flatten();
    let count = down.clone().count() as i32;
    if count == 0 {
        return None;
    }
    let (x, y) = down.fold((0, 0), |(x, y), &(at_x, at_y)| (x + at_x, y + at_y));
    Some((x / count, y / count))
}

fn spread(fingers: &[Option<(i32, i32)>; FINGERS]) -> i32 {
    let (Some((x0, y0)), Some((x1, y1))) = (fingers[0], fingers[1]) else {
        return 0;
    };
    let (dx, dy) = (i64::from(x1 - x0), i64::from(y1 - y0));
    integer_root(dx * dx + dy * dy) as i32
}

fn integer_root(value: i64) -> i64 {
    if value <= 0 {
        return 0;
    }
    let mut root = value;
    let mut next = (root + 1) / 2;
    while next < root {
        root = next;
        next = (root + value / root) / 2;
    }
    root
}

impl Desktop {
    /// Finger `finger` (1 or 2; the first is the pointer) went down, moved or lifted at
    /// (`x`, `y`) (`phase` 0, 1 or 2): the window it landed on gets a `TOUCH` record, and the
    /// fingers together may scroll, pinch or swipe.
    pub fn touch(
        &mut self,
        finger: u8,
        x: u32,
        y: u32,
        phase: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let index = usize::from(finger);
        if index == 0 || index >= FINGERS {
            return Action::None;
        }
        self.note_person_input();
        let at = (x as i32, y as i32);
        if phase == 0 {
            self.gesture.touched[index] = self.content_window_at(x, y);
        }
        if let Some(slot) = self.gesture.touched[index] {
            let (left, top) = self.window_at(slot).content_origin();
            let (cx, cy) = (
                (i64::from(x) - left) as i32,
                (i64::from(y) - top) as i32,
            );
            self.send(
                slot,
                EventRecord::new(event::TOUCH, 0, cx, cy, u32::from(finger), phase),
                events,
            );
        }
        if phase == 2 {
            self.gesture.touched[index] = None;
        }
        self.gesture.fingers[0] = (self.buttons & LEFT_BUTTON != 0)
            .then(|| (self.pointer.0 as i32, self.pointer.1 as i32));
        self.gesture.fingers[index] = match phase {
            2 => None,
            _ => Some(at),
        };
        match phase {
            0 => self.fingers_changed(),
            2 => {
                self.fingers_changed();
                Action::None
            }
            _ => self.fingers_moved(damage, events),
        }
    }

    fn fingers_changed(&mut self) -> Action {
        let down = self.gesture.fingers.iter().flatten().count();
        let first_on_keyboard = self.on_screen_keyboard_holds();
        self.gesture.kind = match (down, self.gesture.kind) {
            (0 | 1, _) => Kind::None,
            _ if first_on_keyboard || self.gesture.fingers[0].is_none() => Kind::None,
            (3, Kind::Swipe { switched }) => Kind::Swipe { switched },
            (_, Kind::Swipe { .. }) => Kind::None,
            (2, Kind::Scroll | Kind::Pinch) => self.gesture.kind,
            _ => Kind::Undecided,
        };
        if self.gesture.kind == Kind::Undecided {
            self.gesture.start_middle = middle(&self.gesture.fingers).unwrap_or_default();
            self.gesture.start_spread = spread(&self.gesture.fingers);
            self.gesture.scrolled = (0, 0);
        }
        Action::None
    }

    fn fingers_moved(&mut self, damage: &mut Damage, events: &mut dyn EventSink) -> Action {
        let Some(now) = middle(&self.gesture.fingers) else {
            return Action::None;
        };
        let moved = (
            now.0 - self.gesture.start_middle.0,
            now.1 - self.gesture.start_middle.1,
        );
        let spread_now = spread(&self.gesture.fingers);
        let spread_change = spread_now - self.gesture.start_spread;
        match self.gesture.kind {
            Kind::Undecided => {
                let far = moved.0.abs().max(moved.1.abs());
                if far < SLOP && spread_change.abs() < SLOP {
                    return Action::None;
                }
                self.take_press(events);
                let three = self.gesture.fingers.iter().flatten().count() == 3;
                self.gesture.kind = match (three, spread_change.abs() > far) {
                    (true, _) => Kind::Swipe { switched: false },
                    (false, true) => Kind::Pinch,
                    (false, false) => Kind::Scroll,
                };
                self.fingers_moved(damage, events)
            }
            Kind::Scroll => {
                let steps = (moved.1 / SCROLL_STEP, moved.0 / SCROLL_STEP);
                let (done_y, done_x) = self.gesture.scrolled;
                if steps.0 != done_y {
                    self.wheel(steps.0 - done_y, events);
                }
                if steps.1 != done_x {
                    self.sideways_wheel(done_x - steps.1, events);
                }
                self.gesture.scrolled = steps;
                Action::None
            }
            Kind::Pinch => {
                let start = i64::from(self.gesture.start_spread.max(1));
                let scale = (i64::from(spread_now) * PINCH_UNIT / start) as u32;
                let target = self.grab.or_else(|| {
                    self.content_window_at(now.0.max(0) as u32, now.1.max(0) as u32)
                });
                if let Some(slot) = target {
                    let (left, top) = self.window_at(slot).content_origin();
                    let (cx, cy) = (
                        (i64::from(now.0) - left) as i32,
                        (i64::from(now.1) - top) as i32,
                    );
                    self.send(
                        slot,
                        EventRecord::new(event::PINCH, 0, cx, cy, scale, 0),
                        events,
                    );
                }
                Action::None
            }
            Kind::Swipe { switched: false } if moved.0.abs() >= SWIPE_DISTANCE => {
                self.gesture.kind = Kind::Swipe { switched: true };
                let (count, shown, _) = self.desktops();
                let wanted = match moved.0 < 0 {
                    true => shown.checked_add(1).filter(|&next| next < count),
                    false => shown.checked_sub(1),
                };
                if let Some(desktop) = wanted {
                    let _ = self.switch_desktop(desktop, damage, events);
                }
                Action::None
            }
            Kind::Swipe { .. } | Kind::None => Action::None,
        }
    }

    fn take_press(&mut self, events: &mut dyn EventSink) {
        if self.gesture.swallow_release || self.buttons & LEFT_BUTTON == 0 {
            return;
        }
        self.gesture.swallow_release = true;
        if let Some(slot) = self.grab {
            self.send(
                slot,
                EventRecord::new(event::PRESS_CANCELLED, 0, 0, 0, 0, 0),
                events,
            );
        }
    }

    /// Whether the left button's release ends a gesture rather than a press: the release is
    /// then the gesture's, and only lets the grab go.
    pub(super) fn release_ends_gesture(&mut self) -> bool {
        core::mem::take(&mut self.gesture.swallow_release)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spread_is_the_distance_between_the_first_two_fingers() {
        let fingers = [Some((0, 0)), Some((30, 40)), None];
        assert_eq!(spread(&fingers), 50);
        assert_eq!(middle(&fingers), Some((15, 20)));
    }

    #[test]
    fn an_integer_root_rounds_down() {
        assert_eq!(integer_root(0), 0);
        assert_eq!(integer_root(1), 1);
        assert_eq!(integer_root(99), 9);
        assert_eq!(integer_root(100), 10);
    }
}
