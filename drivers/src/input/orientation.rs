//! Which way up a device is held, from its accelerometer (D435): the axis gravity pulls along
//! in the screen's plane names the edge that is up, and the picture turns the other way so it
//! stays upright. A reading counts only when one axis clearly leads (a device lying flat, or
//! held at a diagonal, keeps what it shows), and a new way up is taken only after it held for
//! [`STEADY_READINGS`] readings in a row, so a shake does not turn the screen.

/// How many readings in a row a new way up must hold before it is taken.
pub const STEADY_READINGS: u32 = 3;
const LEAD_NUMERATOR: i64 = 3;
const LEAD_DENOMINATOR: i64 = 2;

/// How far the picture must turn clockwise on the device for it to look upright: 0, 90, 180
/// or 270 degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Facing {
    #[default]
    Upright,
    TurnedClockwise,
    UpsideDown,
    TurnedCounterClockwise,
}

impl Facing {
    /// The degrees the picture turns clockwise.
    pub const fn degrees(self) -> u32 {
        match self {
            Self::Upright => 0,
            Self::TurnedClockwise => 90,
            Self::UpsideDown => 180,
            Self::TurnedCounterClockwise => 270,
        }
    }
}

/// The way up one reading shows, if one axis in the screen's plane clearly leads: the
/// acceleration the sensor reports at rest points away from the ground, +y toward the
/// screen's top edge and +x toward its right edge.
pub fn facing_of(x: i32, y: i32, z: i32) -> Option<Facing> {
    let (x, y, z) = (i64::from(x), i64::from(y), i64::from(z));
    let leads = |a: i64, b: i64| a.abs() * LEAD_DENOMINATOR > b.abs() * LEAD_NUMERATOR;
    if leads(z, x) && leads(z, y) {
        return None;
    }
    if leads(y, x) {
        return Some(match y > 0 {
            true => Facing::Upright,
            false => Facing::UpsideDown,
        });
    }
    if leads(x, y) {
        return Some(match x > 0 {
            true => Facing::TurnedCounterClockwise,
            false => Facing::TurnedClockwise,
        });
    }
    None
}

/// The way up taken so far and the one waiting to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OrientationTracker {
    taken: Facing,
    waiting: Option<(Facing, u32)>,
}

impl OrientationTracker {
    /// A tracker that shows the picture upright until a reading says otherwise.
    pub const fn new() -> Self {
        Self {
            taken: Facing::Upright,
            waiting: None,
        }
    }

    /// The way up taken.
    pub fn facing(&self) -> Facing {
        self.taken
    }

    /// Takes a reading: the new way up once it held long enough, `None` while nothing changed.
    pub fn note(&mut self, x: i32, y: i32, z: i32) -> Option<Facing> {
        let Some(facing) = facing_of(x, y, z) else {
            self.waiting = None;
            return None;
        };
        if facing == self.taken {
            self.waiting = None;
            return None;
        }
        let held = match self.waiting {
            Some((waiting, held)) if waiting == facing => held + 1,
            _ => 1,
        };
        if held < STEADY_READINGS {
            self.waiting = Some((facing, held));
            return None;
        }
        self.taken = facing;
        self.waiting = None;
        Some(facing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: i32 = 1000;

    #[test]
    fn the_axis_gravity_leads_along_names_the_way_up() {
        assert_eq!(facing_of(0, G, 0), Some(Facing::Upright));
        assert_eq!(facing_of(0, -G, 0), Some(Facing::UpsideDown));
        assert_eq!(facing_of(-G, 0, 0), Some(Facing::TurnedClockwise));
        assert_eq!(facing_of(G, 0, 0), Some(Facing::TurnedCounterClockwise));
        assert_eq!(Facing::TurnedClockwise.degrees(), 90);
    }

    #[test]
    fn flat_or_diagonal_readings_keep_what_is_shown() {
        assert_eq!(facing_of(0, 0, G), None);
        assert_eq!(facing_of(700, 700, 0), None);
    }

    #[test]
    fn a_new_way_up_must_hold_before_it_is_taken() {
        let mut tracker = OrientationTracker::default();
        assert_eq!(tracker.note(-G, 0, 0), None);
        assert_eq!(tracker.note(-G, 0, 0), None);
        assert_eq!(tracker.note(0, G, 0), None);
        assert_eq!(tracker.note(-G, 0, 0), None);
        assert_eq!(tracker.note(-G, 0, 0), None);
        assert_eq!(tracker.note(-G, 0, 0), Some(Facing::TurnedClockwise));
        assert_eq!(tracker.facing(), Facing::TurnedClockwise);
        assert_eq!(tracker.note(-G, 0, 0), None);
    }
}
