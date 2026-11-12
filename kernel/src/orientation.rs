//! The device's way up, from an accelerometer (D435): readings go through the drivers'
//! [`OrientationTracker`], and a new way up is kept for the desktop's loop, which turns the
//! screen to it while `display.rotation` is `auto` ([`apply_if_automatic`]).

use crate::sync::LeafSpinLock;
use core::sync::atomic::{AtomicU32, Ordering};
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

/// Turns the screen to a new way up, when one was taken and `display.rotation` is `auto`;
/// called from the desktop's loop.
pub fn apply_if_automatic<P: Platform>() {
    let degrees = NEW_DEGREES.swap(NOTHING_NEW, Ordering::AcqRel);
    if degrees == NOTHING_NEW || crate::terminal::rotation_setting() != AUTOMATIC {
        return;
    }
    crate::terminal::turn_screen::<P>(degrees);
}

const AUTOMATIC: u32 = 0;
