//! Input devices' events as they arrive: the event types and codes keyboards, mice and tablets
//! report (the Linux input event codes, which virtio-input carries unchanged and USB HID maps
//! to), what a device can report, the US keymap ([`keymap`]) and the pointer's scaling.

pub mod keymap;

#[cfg(test)]
mod tests;

/// One event as a device reports it: a type, a code and a value. A `SYN_REPORT` ends each
/// report; the events before it happened together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawEvent {
    /// The event type (see [`event_type`]).
    pub kind: u16,
    /// What it is about: a key, a button or an axis.
    pub code: u16,
    /// For a key: 1 pressed, 0 released, 2 repeated; for an axis: the position or the motion.
    pub value: i32,
}

impl RawEvent {
    /// The event of type `kind` with `code` and `value`.
    pub const fn new(kind: u16, code: u16, value: i32) -> Self {
        Self { kind, code, value }
    }
}

/// Event types.
pub mod event_type {
    /// The end of a report ([`super::SYN_REPORT`]).
    pub const SYN: u16 = 0x00;
    /// A key or a button.
    pub const KEY: u16 = 0x01;
    /// Relative motion.
    pub const REL: u16 = 0x02;
    /// An absolute position.
    pub const ABS: u16 = 0x03;
    /// Autorepeat: a device that lists it leaves repeating to the driver.
    pub const REP: u16 = 0x14;
}

/// The code of a [`event_type::SYN`] event that ends a report.
pub const SYN_REPORT: u16 = 0;

/// Key codes the keymap knows, and the highest one.
pub mod key {
    pub const ESC: u16 = 1;
    pub const MINUS: u16 = 12;
    pub const BACKSPACE: u16 = 14;
    pub const TAB: u16 = 15;
    pub const W: u16 = 17;
    pub const ENTER: u16 = 28;
    pub const LEFT_CTRL: u16 = 29;
    pub const A: u16 = 30;
    pub const D: u16 = 32;
    pub const H: u16 = 35;
    pub const LEFT_SHIFT: u16 = 42;
    pub const X: u16 = 45;
    pub const M: u16 = 50;
    pub const RIGHT_SHIFT: u16 = 54;
    pub const LEFT_ALT: u16 = 56;
    pub const SPACE: u16 = 57;
    pub const CAPS_LOCK: u16 = 58;
    pub const F1: u16 = 59;
    pub const F2: u16 = 60;
    pub const F3: u16 = 61;
    pub const F4: u16 = 62;
    pub const F5: u16 = 63;
    pub const F6: u16 = 64;
    pub const F7: u16 = 65;
    pub const F8: u16 = 66;
    pub const F9: u16 = 67;
    pub const F10: u16 = 68;
    pub const NUM_LOCK: u16 = 69;
    pub const SCROLL_LOCK: u16 = 70;
    pub const KEYPAD_DOT: u16 = 83;
    pub const F11: u16 = 87;
    pub const F12: u16 = 88;
    pub const KEYPAD_ENTER: u16 = 96;
    pub const RIGHT_CTRL: u16 = 97;
    pub const RIGHT_ALT: u16 = 100;
    pub const HOME: u16 = 102;
    pub const UP: u16 = 103;
    pub const PAGE_UP: u16 = 104;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const END: u16 = 107;
    pub const DOWN: u16 = 108;
    pub const PAGE_DOWN: u16 = 109;
    pub const INSERT: u16 = 110;
    pub const DELETE: u16 = 111;
    pub const LEFT_SUPER: u16 = 125;
    pub const RIGHT_SUPER: u16 = 126;
    pub const COMPOSE: u16 = 127;
    /// The highest key or button code.
    pub const MAX: u16 = 0x2ff;
}

/// Pointer button codes (reported as [`event_type::KEY`] events).
pub mod button {
    /// The first button code: the codes below are keyboard keys.
    pub const FIRST: u16 = 0x100;
    pub const LEFT: u16 = 0x110;
    pub const RIGHT: u16 = 0x111;
    pub const MIDDLE: u16 = 0x112;
}

/// Relative axes.
pub mod relative {
    pub const X: u16 = 0x00;
    pub const Y: u16 = 0x01;
    /// The vertical wheel: +1 a notch up.
    pub const WHEEL: u16 = 0x08;
}

/// Absolute axes.
pub mod absolute {
    pub const X: u16 = 0x00;
    pub const Y: u16 = 0x01;
}

/// The values an absolute axis reports, from `min` to `max`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AxisRange {
    pub min: i32,
    pub max: i32,
}

/// What a device reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Keyboard keys (any of the codes from Esc to F12).
    pub keyboard: bool,
    /// The left pointer button.
    pub buttons: bool,
    /// Relative motion on X and Y: a mouse.
    pub relative: bool,
    /// The vertical wheel.
    pub wheel: bool,
    /// Absolute positions on X and Y and their ranges: a tablet or a touch screen.
    pub absolute: Option<(AxisRange, AxisRange)>,
}

impl Capabilities {
    /// Whether the device moves a pointer.
    pub const fn is_pointer(&self) -> bool {
        self.relative || self.absolute.is_some()
    }
}

/// The pixel, from 0 to `size - 1`, that `value` on an axis of `range` stands for: rounded to
/// the nearest, so a position a host scaled down from a pixel (as QEMU's windows do, truncating)
/// comes back to that pixel. Values outside the range are clamped; 0 for an empty screen or
/// range.
pub const fn scale_to_screen(value: i32, range: AxisRange, size: u32) -> u32 {
    if size == 0 || range.max <= range.min {
        return 0;
    }
    let span = range.max.abs_diff(range.min) as u64;
    let offset = if value < range.min {
        0
    } else if value > range.max {
        span
    } else {
        value.abs_diff(range.min) as u64
    };
    let pixel = (offset * size as u64 + span / 2) / span;
    if pixel >= size as u64 {
        size - 1
    } else {
        pixel as u32
    }
}
