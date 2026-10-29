//! `NSEvent`'s `clickCount` (D162): presses of the left button at nearly the same place, each
//! within [`DOUBLE_CLICK_MS`] of the one before, count up; any other press starts again at 1.

/// The longest time between two presses that count together, in ms (AppKit's default
/// `doubleClickInterval`).
pub const DOUBLE_CLICK_MS: u32 = 500;
/// The farthest a press may be from the one before, each way, in pixels, to count with it.
pub const DOUBLE_CLICK_DISTANCE: u32 = 4;

/// The left button's presses so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickCounter {
    last: Option<(u32, i32, i32)>,
    count: isize,
}

impl ClickCounter {
    /// A counter that has seen no press.
    pub const fn new() -> Self {
        Self {
            last: None,
            count: 0,
        }
    }

    /// A press at (`x`, `y`) at `time_ms`: its click count.
    pub fn press(&mut self, time_ms: u32, x: i32, y: i32) -> isize {
        let follows = self.last.is_some_and(|(at, last_x, last_y)| {
            time_ms.wrapping_sub(at) <= DOUBLE_CLICK_MS
                && x.abs_diff(last_x) <= DOUBLE_CLICK_DISTANCE
                && y.abs_diff(last_y) <= DOUBLE_CLICK_DISTANCE
        });
        self.count = match follows {
            true => self.count.saturating_add(1),
            false => 1,
        };
        self.last = Some((time_ms, x, y));
        self.count
    }

    /// The click count of the release that ends the last press (1 before any press).
    pub fn release(&self) -> isize {
        self.count.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_close_in_time_and_place_count_up() {
        let mut clicks = ClickCounter::new();
        assert_eq!(clicks.release(), 1);
        assert_eq!(clicks.press(1000, 10, 10), 1);
        assert_eq!(clicks.release(), 1);
        assert_eq!(
            clicks.press(1000, 10, 10),
            2,
            "an agent's double-click: same time"
        );
        assert_eq!(clicks.release(), 2);
        assert_eq!(clicks.press(1500, 14, 6), 3);
        assert_eq!(clicks.press(2001, 14, 6), 1, "too late");
        assert_eq!(clicks.press(2002, 19, 6), 1, "too far");
        assert_eq!(clicks.press(u32::MAX - 10, 0, 0), 1);
        assert_eq!(clicks.press(100, 0, 0), 2, "across the clock's wrap");
    }
}
