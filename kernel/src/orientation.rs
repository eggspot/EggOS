//! The device's way up, from an accelerometer (D435): readings go through the drivers'
//! [`OrientationTracker`], and a new way up is kept for the desktop's loop, which turns the
//! screen to it while `display.rotation` is `auto` ([`apply_if_automatic`]).

use crate::sync::LeafSpinLock;
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_app_manifest::Orientation;
use eggos_drivers::input::orientation::{Facing, OrientationTracker};
use eggos_hal::Platform;

const NOTHING_NEW: u32 = u32::MAX;

static TRACKER: LeafSpinLock<OrientationTracker> = LeafSpinLock::new_leaf(OrientationTracker::new());
static NEW_DEGREES: AtomicU32 = AtomicU32::new(NOTHING_NEW);
static SENSED: AtomicU32 = AtomicU32::new(0);

/// Takes an accelerometer reading.
pub fn note(x: i32, y: i32, z: i32) {
    SENSED.store(1, Ordering::Relaxed);
    if let Some(facing) = TRACKER.lock().note(x, y, z) {
        NEW_DEGREES.store(facing.degrees(), Ordering::Release);
    }
}

/// Whether an accelerometer has reported since boot.
pub fn has_sensor() -> bool {
    SENSED.load(Ordering::Relaxed) != 0
}

/// The way up taken last.
pub fn facing() -> Facing {
    TRACKER.lock().facing()
}

/// Turns the screen to the way up the sensor took, while `display.rotation` is `auto`, unless
/// the focused app holds the other orientation (D436): then the screen stays as it is, and
/// turns once the app lets go or the device comes round. Called from the desktop's loop.
pub fn apply_if_automatic<P: Platform>() {
    let fresh = NEW_DEGREES.swap(NOTHING_NEW, Ordering::AcqRel);
    if fresh != NOTHING_NEW {
        WANTED_DEGREES.store(fresh, Ordering::Relaxed);
    }
    let wanted = WANTED_DEGREES.load(Ordering::Relaxed);
    if wanted == NOTHING_NEW || crate::terminal::rotation_setting() != AUTOMATIC {
        return;
    }
    let Some(rotation) = crate::display::Rotation::of_degrees(wanted) else {
        return;
    };
    if rotation == crate::display::rotation() || fresh == NOTHING_NEW && !due::<P>() {
        return;
    }
    let held = crate::terminal::with_windows::<P, _>(|desktop, _, _, _| desktop.focused_orientation())
        .unwrap_or_default();
    let fits = match (held, crate::display::device_mode()) {
        (Orientation::Any, _) | (_, None) => true,
        (wanted_shape, Some(mode)) => {
            let portrait = (mode.height > mode.width) != rotation.swaps();
            portrait == (wanted_shape == Orientation::Portrait)
        }
    };
    if fits {
        crate::terminal::turn_screen::<P>(wanted);
    }
}

static WANTED_DEGREES: AtomicU32 = AtomicU32::new(NOTHING_NEW);

const AUTOMATIC: u32 = 0;

static LOOKED_AT: eggos_hal::atomic::AtomicU64 = eggos_hal::atomic::AtomicU64::new(0);
const LOOK_EVERY_MS: u64 = 500;

fn due<P: Platform>() -> bool {
    let now = P::clock_ticks();
    let every = eggos_hal::time::mul_div_ceil(LOOK_EVERY_MS, P::clock_frequency(), 1000);
    let last = LOOKED_AT.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < every {
        return false;
    }
    LOOKED_AT.store(now.max(1), Ordering::Relaxed);
    true
}
