//! HID report descriptors and the input reports they describe (Device Class Definition for HID
//! 1.11, 6.2.2; HID Usage Tables 1.4; D256): a descriptor parsed into the fields of its input
//! reports, and reports decoded into the kernel's input events, for keyboards, mice, tablets,
//! touch screens and media keys that speak the report protocol rather than the boot protocol.

use super::hid::{BootKeyboard, KEYBOARD_REPORT_BYTES};
use super::{AxisRange, Capabilities, RawEvent, SYN_REPORT, absolute, button, event_type, relative};

pub const MAX_FIELDS: usize = 48;
const MAX_DEPTH: usize = 4;

const PAGE_GENERIC_DESKTOP: u16 = 0x01;
const PAGE_KEYBOARD: u16 = 0x07;
const PAGE_BUTTON: u16 = 0x09;
const PAGE_CONSUMER: u16 = 0x0c;
const PAGE_DIGITIZER: u16 = 0x0d;
const PAGE_SENSOR: u16 = 0x20;
const USAGE_ACCELERATION_X: u16 = 0x0453;
const USAGE_ACCELERATION_Z: u16 = 0x0455;
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_WHEEL: u16 = 0x38;
const USAGE_TIP_SWITCH: u16 = 0x42;
const USAGE_CONSUMER_PAN: u16 = 0x238;
const FIRST_MODIFIER: u16 = 0xe0;
const LAST_MODIFIER: u16 = 0xe7;

const FLAG_CONSTANT: u32 = 1;
const FLAG_VARIABLE: u32 = 1 << 1;
const FLAG_RELATIVE: u32 = 1 << 2;

/// One field of an input report: `count` values of `size` bits from `bit` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Field {
    pub report_id: u8,
    pub bit: u32,
    pub size: u32,
    pub count: u32,
    pub page: u16,
    /// The usage of the first value; a variable field's values take consecutive usages from it
    /// (or from `usage_minimum`), an array field's values are usages from `usage_minimum`.
    pub usage: u16,
    pub usage_minimum: u16,
    pub usage_maximum: u16,
    pub logical_minimum: i32,
    pub logical_maximum: i32,
    pub flags: u32,
}

impl Field {
    fn is_variable(&self) -> bool {
        self.flags & FLAG_VARIABLE != 0
    }

    fn is_relative(&self) -> bool {
        self.flags & FLAG_RELATIVE != 0
    }

    fn usage_of(&self, index: u32) -> u16 {
        if self.usage_maximum > self.usage_minimum || self.usage_minimum != 0 {
            self.usage_minimum.saturating_add(index as u16)
        } else {
            self.usage.saturating_add(index as u16)
        }
    }
}

/// The input fields a report descriptor declares.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    fields: [Field; MAX_FIELDS],
    count: usize,
    pub has_report_ids: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            fields: [Field::default(); MAX_FIELDS],
            count: 0,
            has_report_ids: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Globals {
    page: u16,
    logical_minimum: i32,
    logical_maximum: i32,
    size: u32,
    count: u32,
    report_id: u8,
}

fn unsigned(data: &[u8]) -> u32 {
    data.iter().rev().fold(0, |value, &byte| (value << 8) | u32::from(byte))
}

fn signed(data: &[u8]) -> i32 {
    match data.len() {
        1 => i32::from(data[0] as i8),
        2 => i32::from(i16::from_le_bytes([data[0], data[1]])),
        4 => i32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        _ => 0,
    }
}

impl Layout {
    /// Parses a report descriptor; `None` when it is malformed.
    pub fn parse(descriptor: &[u8]) -> Option<Self> {
        let mut layout = Self::default();
        let mut globals = Globals::default();
        let mut stack = [Globals::default(); MAX_DEPTH];
        let mut depth = 0;
        let mut usages = [0u16; 16];
        let mut usage_count = 0;
        let mut usage_minimum = 0u16;
        let mut usage_maximum = 0u16;
        let mut offsets = [0u32; 256];
        let mut at = 0;
        while at < descriptor.len() {
            let prefix = descriptor[at];
            if prefix == 0xfe {
                let length = usize::from(*descriptor.get(at + 1)?);
                at += 3 + length;
                continue;
            }
            let size = match prefix & 0b11 {
                3 => 4,
                size => usize::from(size),
            };
            let data = descriptor.get(at + 1..at + 1 + size)?;
            let kind = (prefix >> 2) & 0b11;
            let tag = prefix >> 4;
            match (kind, tag) {
                (0, 0x8) => {
                    let flags = unsigned(data);
                    let id = usize::from(globals.report_id);
                    let start = offsets[id];
                    let bits = globals.size.saturating_mul(globals.count);
                    offsets[id] = start.saturating_add(bits);
                    if flags & FLAG_CONSTANT == 0 {
                        let template = Field {
                            report_id: globals.report_id,
                            bit: start,
                            size: globals.size,
                            count: globals.count,
                            page: globals.page,
                            usage: if usage_count > 0 { usages[0] } else { 0 },
                            usage_minimum,
                            usage_maximum,
                            logical_minimum: globals.logical_minimum,
                            logical_maximum: globals.logical_maximum,
                            flags,
                        };
                        let split = flags & FLAG_VARIABLE != 0 && usage_count > 1;
                        let pieces = if split { usage_count.min(globals.count as usize) } else { 1 };
                        for piece in 0..pieces {
                            if layout.count == MAX_FIELDS {
                                break;
                            }
                            let mut field = template;
                            if split {
                                field.bit = start + piece as u32 * globals.size;
                                field.count = 1;
                                field.usage = usages[piece];
                                field.usage_minimum = 0;
                                field.usage_maximum = 0;
                            }
                            layout.fields[layout.count] = field;
                            layout.count += 1;
                        }
                    }
                    usage_count = 0;
                    usage_minimum = 0;
                    usage_maximum = 0;
                }
                (0, 0x9) | (0, 0xb) | (0, 0xa) | (0, 0xc) => {
                    usage_count = 0;
                    usage_minimum = 0;
                    usage_maximum = 0;
                }
                (1, 0x0) => globals.page = unsigned(data) as u16,
                (1, 0x1) => globals.logical_minimum = signed(data),
                (1, 0x2) => globals.logical_maximum = signed(data),
                (1, 0x7) => globals.size = unsigned(data).min(32),
                (1, 0x8) => {
                    globals.report_id = unsigned(data) as u8;
                    layout.has_report_ids = true;
                }
                (1, 0x9) => globals.count = unsigned(data).min(255),
                (1, 0xa) => {
                    *stack.get_mut(depth)? = globals;
                    depth += 1;
                }
                (1, 0xb) => {
                    depth = depth.checked_sub(1)?;
                    globals = stack[depth];
                }
                (2, 0x0) => {
                    if size == 4 {
                        globals.page = (unsigned(data) >> 16) as u16;
                    }
                    if usage_count < usages.len() {
                        usages[usage_count] = unsigned(data) as u16;
                        usage_count += 1;
                    }
                }
                (2, 0x1) => usage_minimum = unsigned(data) as u16,
                (2, 0x2) => usage_maximum = unsigned(data) as u16,
                _ => {}
            }
            at += 1 + size;
        }
        Some(layout)
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields[..self.count]
    }

    fn find(&self, page: u16, usage: u16) -> Option<&Field> {
        self.fields()
            .iter()
            .find(|field| field.page == page && field.is_variable() && field.usage_of(0) == usage)
    }

    /// Writes into `out` the distinct ids of the input reports that carry an accelerometer's
    /// axes (D435), as many as fit: what a polled sensor is asked for (D462). How many.
    pub fn sensor_reports(&self, out: &mut [u8]) -> usize {
        let mut count = 0;
        let axes = self.fields().iter().filter(|field| {
            field.page == PAGE_SENSOR
                && field.is_variable()
                && (0..field.count).any(|index| {
                    (USAGE_ACCELERATION_X..=USAGE_ACCELERATION_Z).contains(&field.usage_of(index))
                })
        });
        for field in axes {
            if count < out.len() && !out[..count].contains(&field.report_id) {
                out[count] = field.report_id;
                count += 1;
            }
        }
        count
    }

    /// What a device with this layout reports.
    pub fn capabilities(&self) -> Capabilities {
        let keyboard = self.fields().iter().any(|field| field.page == PAGE_KEYBOARD);
        let x = self.find(PAGE_GENERIC_DESKTOP, USAGE_X);
        let y = self.find(PAGE_GENERIC_DESKTOP, USAGE_Y);
        let buttons = self.fields().iter().any(|field| field.page == PAGE_BUTTON)
            || self.find(PAGE_DIGITIZER, USAGE_TIP_SWITCH).is_some();
        let relative = x.is_some_and(Field::is_relative);
        let absolute = match (x, y) {
            (Some(x), Some(y)) if !x.is_relative() => Some((
                AxisRange {
                    min: x.logical_minimum,
                    max: x.logical_maximum,
                },
                AxisRange {
                    min: y.logical_minimum,
                    max: y.logical_maximum,
                },
            )),
            _ => None,
        };
        Capabilities {
            keyboard,
            buttons,
            relative,
            wheel: self.find(PAGE_GENERIC_DESKTOP, USAGE_WHEEL).is_some(),
            absolute,
        }
    }
}

fn extract(report: &[u8], bit: u32, size: u32) -> u32 {
    let mut value = 0u32;
    for index in 0..size.min(32) {
        let position = bit + index;
        let byte = report.get((position / 8) as usize).copied().unwrap_or(0);
        if byte >> (position % 8) & 1 != 0 {
            value |= 1 << index;
        }
    }
    value
}

fn sign_extend(value: u32, size: u32, field: &Field) -> i32 {
    if field.logical_minimum < 0 && size > 0 && size < 32 && value >> (size - 1) & 1 != 0 {
        (value | (u32::MAX << size)) as i32
    } else {
        value as i32
    }
}

fn consumer_key(usage: u16) -> u16 {
    match usage {
        0xe2 => 113,
        0xea => 114,
        0xe9 => 115,
        0xcd => 164,
        0xb5 => 163,
        0xb6 => 165,
        0xb7 => 166,
        0x6f => 225,
        0x70 => 224,
        _ => 0,
    }
}

/// A device's decoding state: the keyboard's keys and the buttons and keys held.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decoder {
    keyboard: BootKeyboard,
    buttons: u8,
    consumer: u16,
}

impl Decoder {
    /// Hands `event` what report `report` (with its report id first when the layout has ids)
    /// changed, then a report's end; returns how many events it sent.
    pub fn report(&mut self, layout: &Layout, report: &[u8], event: &mut dyn FnMut(RawEvent)) -> usize {
        let (id, body) = match (layout.has_report_ids, report.split_first()) {
            (true, Some((&id, body))) => (id, body),
            (true, None) => return 0,
            (false, _) => (0, report),
        };
        let mut sent = 0;
        let mut send = |raw: RawEvent| {
            event(raw);
            sent += 1;
        };
        let mut boot = [0u8; KEYBOARD_REPORT_BYTES];
        let mut keys = 0;
        let mut keyboard_seen = false;
        let mut buttons = self.buttons;
        let mut buttons_seen = false;
        let mut consumer_seen = None;
        for field in layout.fields().iter().filter(|field| field.report_id == id) {
            for index in 0..field.count {
                let raw = extract(body, field.bit + index * field.size, field.size);
                let value = sign_extend(raw, field.size, field);
                let usage = if field.is_variable() { field.usage_of(index) } else { field.usage_minimum.saturating_add(raw as u16) };
                match (field.page, field.is_variable()) {
                    (PAGE_SENSOR, true)
                        if (USAGE_ACCELERATION_X..=USAGE_ACCELERATION_Z).contains(&usage) =>
                    {
                        let axis = absolute::ACCELERATION_X + (usage - USAGE_ACCELERATION_X);
                        send(RawEvent::new(event_type::ABS, axis, value));
                    }
                    (PAGE_KEYBOARD, true) if (FIRST_MODIFIER..=LAST_MODIFIER).contains(&usage) => {
                        keyboard_seen = true;
                        if value != 0 {
                            boot[0] |= 1 << (usage - FIRST_MODIFIER);
                        }
                    }
                    (PAGE_KEYBOARD, true) => {
                        keyboard_seen = true;
                        if value != 0 && keys < 6 && usage <= 0xff {
                            boot[2 + keys] = usage as u8;
                            keys += 1;
                        }
                    }
                    (PAGE_KEYBOARD, false) => {
                        keyboard_seen = true;
                        if raw != 0 && keys < 6 && usage <= 0xff {
                            boot[2 + keys] = usage as u8;
                            keys += 1;
                        }
                    }
                    (PAGE_BUTTON, true) if (1..=3).contains(&usage) => {
                        buttons_seen = true;
                        let bit = 1 << (usage - 1);
                        if value != 0 { buttons |= bit } else { buttons &= !bit }
                    }
                    (PAGE_DIGITIZER, true) if usage == USAGE_TIP_SWITCH => {
                        buttons_seen = true;
                        if value != 0 { buttons |= 1 } else { buttons &= !1 }
                    }
                    (PAGE_GENERIC_DESKTOP, true) if matches!(usage, USAGE_X | USAGE_Y | USAGE_WHEEL) => {
                        let relative_field = field.is_relative();
                        let (kind, code) = match (usage, relative_field) {
                            (USAGE_X, true) => (event_type::REL, relative::X),
                            (USAGE_Y, true) => (event_type::REL, relative::Y),
                            (USAGE_X, false) => (event_type::ABS, absolute::X),
                            (USAGE_Y, false) => (event_type::ABS, absolute::Y),
                            _ => (event_type::REL, relative::WHEEL),
                        };
                        if !relative_field || value != 0 {
                            send(RawEvent { kind, code, value });
                        }
                    }
                    (PAGE_CONSUMER, true) if usage == USAGE_CONSUMER_PAN && value != 0 => {
                        send(RawEvent {
                            kind: event_type::REL,
                            code: relative::SIDEWAYS_WHEEL,
                            value,
                        });
                    }
                    (PAGE_CONSUMER, _) => {
                        let pressed = if field.is_variable() { (value != 0).then_some(usage) } else { (raw != 0).then_some(usage) };
                        if pressed.is_some() || consumer_seen.is_none() {
                            consumer_seen = Some(pressed.unwrap_or(0));
                        }
                    }
                    _ => {}
                }
            }
        }
        if keyboard_seen {
            sent += self.keyboard.report(&boot, &mut |raw| event(raw));
        }
        if buttons_seen {
            for (bit, code) in [button::LEFT, button::RIGHT, button::MIDDLE].into_iter().enumerate() {
                let mask = 1 << bit;
                if self.buttons & mask != buttons & mask {
                    event(RawEvent {
                        kind: event_type::KEY,
                        code,
                        value: i32::from(buttons & mask != 0),
                    });
                    sent += 1;
                }
            }
            self.buttons = buttons;
        }
        if let Some(usage) = consumer_seen {
            if usage != self.consumer {
                for (code, pressed) in [(consumer_key(self.consumer), false), (consumer_key(usage), true)] {
                    if code != 0 {
                        event(RawEvent {
                            kind: event_type::KEY,
                            code,
                            value: i32::from(pressed),
                        });
                        sent += 1;
                    }
                }
                self.consumer = usage;
            }
        }
        if sent > 0 {
            event(RawEvent {
                kind: event_type::SYN,
                code: SYN_REPORT,
                value: 0,
            });
            sent += 1;
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUSE: [u8; 50] = [
        0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29,
        0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
        0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95,
        0x02, 0x81, 0x06, 0xc0, 0xc0,
    ];

    #[test]
    fn a_report_protocol_mouse_moves_and_clicks() {
        let layout = Layout::parse(&MOUSE).unwrap();
        let capabilities = layout.capabilities();
        assert!(capabilities.relative && capabilities.buttons);
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        decoder.report(&layout, &[0x01, 0x05, 0xfe], &mut |event| events.push(event));
        assert!(events.contains(&RawEvent { kind: event_type::REL, code: relative::X, value: 5 }));
        assert!(events.contains(&RawEvent { kind: event_type::REL, code: relative::Y, value: -2 }));
        assert!(events.contains(&RawEvent { kind: event_type::KEY, code: button::LEFT, value: 1 }));
    }

    #[test]
    fn a_touch_screen_reports_absolute_positions_and_its_tip() {
        let descriptor = [
            0x05, 0x0d, 0x09, 0x04, 0xa1, 0x01, 0x85, 0x02, 0x09, 0x42, 0x15, 0x00, 0x25, 0x01,
            0x75, 0x01, 0x95, 0x01, 0x81, 0x02, 0x95, 0x07, 0x81, 0x03, 0x05, 0x01, 0x26, 0xff,
            0x0f, 0x75, 0x10, 0x95, 0x01, 0x09, 0x30, 0x81, 0x02, 0x09, 0x31, 0x81, 0x02, 0xc0,
        ];
        let layout = Layout::parse(&descriptor).unwrap();
        assert!(layout.has_report_ids);
        let capabilities = layout.capabilities();
        assert_eq!(capabilities.absolute.map(|(x, _)| x.max), Some(4095));
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        decoder.report(&layout, &[0x02, 0x01, 0x00, 0x08, 0x00, 0x04], &mut |event| events.push(event));
        assert!(events.contains(&RawEvent { kind: event_type::ABS, code: absolute::X, value: 0x800 }));
        assert!(events.contains(&RawEvent { kind: event_type::ABS, code: absolute::Y, value: 0x400 }));
        assert!(events.contains(&RawEvent { kind: event_type::KEY, code: button::LEFT, value: 1 }));
    }

    #[test]
    fn a_report_protocol_keyboard_presses_its_keys() {
        let descriptor = [
            0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00,
            0x25, 0x65, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xc0,
        ];
        let layout = Layout::parse(&descriptor).unwrap();
        assert!(layout.capabilities().keyboard);
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        decoder.report(&layout, &[0x02, 0x04, 0, 0, 0, 0, 0], &mut |event| events.push(event));
        assert!(events.contains(&RawEvent { kind: event_type::KEY, code: 42, value: 1 }));
        assert!(events.contains(&RawEvent { kind: event_type::KEY, code: 30, value: 1 }));
    }
}
