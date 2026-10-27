//! What changed in a window since its last commit: at most [`MAX_RECTS`] rectangles, the most
//! a `COMMIT` names (docs/windows.md), so what is drawn again and copied stays small.

use crate::geometry::Rect;

/// The most rectangles a commit carries.
pub const MAX_RECTS: usize = 8;

/// The rectangles that changed, at scale 1.
#[derive(Debug, Clone, Copy, Default)]
pub struct Damage {
    rects: [Rect; MAX_RECTS],
    count: usize,
}

fn area(rect: &Rect) -> i64 {
    i64::from(rect.width.max(0)) * i64::from(rect.height.max(0))
}

impl Damage {
    /// Nothing changed.
    pub const fn new() -> Self {
        Self {
            rects: [Rect::new(0, 0, 0, 0); MAX_RECTS],
            count: 0,
        }
    }

    /// Adds `rect`: dropped when an earlier one holds it; merged with the one it grows least
    /// when the list is full.
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        if self.rects[..self.count]
            .iter()
            .any(|held| held.union(&rect) == *held)
        {
            return;
        }
        if self.count < MAX_RECTS {
            self.rects[self.count] = rect;
            self.count += 1;
            return;
        }
        let closest = (0..self.count)
            .min_by_key(|&index| {
                let held = &self.rects[index];
                area(&held.union(&rect)) - area(held)
            })
            .unwrap_or(0);
        self.rects[closest] = self.rects[closest].union(&rect);
    }

    /// The rectangles.
    pub fn rects(&self) -> &[Rect] {
        &self.rects[..self.count]
    }

    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Forgets every rectangle.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// The rectangles, forgotten here.
    pub fn take(&mut self) -> Self {
        let taken = *self;
        self.clear();
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_are_kept_apart_until_the_list_is_full_then_merged() {
        let mut damage = Damage::new();
        damage.add(Rect::new(0, 0, 10, 10));
        damage.add(Rect::new(2, 2, 3, 3));
        damage.add(Rect::default());
        assert_eq!(damage.rects().len(), 1);
        for index in 1..MAX_RECTS as i32 {
            damage.add(Rect::new(index * 100, 0, 10, 10));
        }
        assert_eq!(damage.rects().len(), MAX_RECTS);
        damage.add(Rect::new(705, 0, 10, 10));
        assert_eq!(damage.rects().len(), MAX_RECTS);
        assert!(damage.rects().contains(&Rect::new(700, 0, 15, 10)));
        let taken = damage.take();
        assert!(damage.is_empty());
        assert_eq!(taken.rects().len(), MAX_RECTS);
    }
}
