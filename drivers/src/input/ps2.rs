//! The PC's 8042 keyboard controller with a PS/2 keyboard and mouse (D239): scan code set 1 as
//! the controller translates set 2 into, and the standard and IntelliMouse wheel packets, turned
//! into the kernel's input events. Laptops' built-in keyboards and touchpads are usually here.

use super::{RawEvent, SYN_REPORT, button, event_type, relative};
use crate::io::RegisterIo;

/// Offsets of the data port (60h) and the status and command port (64h) from 60h.
pub const DATA: usize = 0;
pub const STATUS: usize = 4;
pub const REGISTER_BLOCK_SIZE: usize = 5;

const STATUS_OUTPUT_FULL: u8 = 1;
const STATUS_INPUT_FULL: u8 = 1 << 1;
const STATUS_FROM_MOUSE: u8 = 1 << 5;
const COMMAND_READ_CONFIGURATION: u8 = 0x20;
const COMMAND_WRITE_CONFIGURATION: u8 = 0x60;
const COMMAND_DISABLE_MOUSE: u8 = 0xa7;
const COMMAND_ENABLE_MOUSE: u8 = 0xa8;
const COMMAND_TEST_MOUSE_PORT: u8 = 0xa9;
const COMMAND_DISABLE_KEYBOARD: u8 = 0xad;
const COMMAND_ENABLE_KEYBOARD: u8 = 0xae;
const COMMAND_TO_MOUSE: u8 = 0xd4;
const CONFIGURATION_KEYBOARD_INTERRUPT: u8 = 1;
const CONFIGURATION_MOUSE_INTERRUPT: u8 = 1 << 1;
const CONFIGURATION_TRANSLATION: u8 = 1 << 6;
const DEVICE_ACKNOWLEDGE: u8 = 0xfa;
const DEVICE_ENABLE_REPORTING: u8 = 0xf4;
const DEVICE_SET_DEFAULTS: u8 = 0xf6;
const DEVICE_SET_SAMPLE_RATE: u8 = 0xf3;
const DEVICE_GET_ID: u8 = 0xf2;
const KEYBOARD_SET_LIGHTS: u8 = 0xed;
const WHEEL_MOUSE_ID: u8 = 3;
const POLLS: usize = 100_000;

/// Which PS/2 devices answered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Found {
    pub keyboard: bool,
    pub mouse: bool,
    pub wheel: bool,
}

/// The 8042 controller and what its devices sent so far.
#[derive(Debug)]
pub struct I8042<R: RegisterIo> {
    regs: R,
    keyboard: Set1Decoder,
    mouse: MousePackets,
    found: Found,
}

impl<R: RegisterIo> I8042<R> {
    pub fn new(regs: R) -> Self {
        Self {
            regs,
            keyboard: Set1Decoder::default(),
            mouse: MousePackets::default(),
            found: Found::default(),
        }
    }

    fn wait_to_write(&self) -> bool {
        (0..POLLS).any(|_| self.regs.read8(STATUS) & STATUS_INPUT_FULL == 0)
    }

    fn read_answer(&self) -> Option<u8> {
        (0..POLLS)
            .find(|_| self.regs.read8(STATUS) & STATUS_OUTPUT_FULL != 0)
            .map(|_| self.regs.read8(DATA))
    }

    fn command(&self, command: u8) -> bool {
        let ready = self.wait_to_write();
        if ready {
            self.regs.write8(STATUS, command);
        }
        ready
    }

    fn write_data(&self, value: u8) -> bool {
        let ready = self.wait_to_write();
        if ready {
            self.regs.write8(DATA, value);
        }
        ready
    }

    fn to_keyboard(&self, value: u8) -> bool {
        self.write_data(value) && self.read_answer() == Some(DEVICE_ACKNOWLEDGE)
    }

    fn to_mouse(&self, value: u8) -> bool {
        self.command(COMMAND_TO_MOUSE)
            && self.write_data(value)
            && self.read_answer() == Some(DEVICE_ACKNOWLEDGE)
    }

    fn drain(&self) {
        for _ in 0..64 {
            if self.regs.read8(STATUS) & STATUS_OUTPUT_FULL == 0 {
                return;
            }
            self.regs.read8(DATA);
        }
    }

    /// Sets the controller up for polling (its interrupts off, translation on) and turns on the
    /// keyboard and, where one answers, the mouse with its wheel.
    pub fn start(&mut self) -> Found {
        if self.regs.read8(STATUS) == 0xff {
            return Found::default();
        }
        self.command(COMMAND_DISABLE_KEYBOARD);
        self.command(COMMAND_DISABLE_MOUSE);
        self.drain();
        let configuration = if self.command(COMMAND_READ_CONFIGURATION) {
            self.read_answer().unwrap_or(0)
        } else {
            return Found::default();
        };
        let configuration = (configuration | CONFIGURATION_TRANSLATION)
            & !(CONFIGURATION_KEYBOARD_INTERRUPT | CONFIGURATION_MOUSE_INTERRUPT);
        self.command(COMMAND_WRITE_CONFIGURATION);
        self.write_data(configuration);
        self.command(COMMAND_ENABLE_KEYBOARD);
        self.found.keyboard = self.to_keyboard(DEVICE_ENABLE_REPORTING);
        let mouse_port_ok = self.command(COMMAND_TEST_MOUSE_PORT) && self.read_answer() == Some(0);
        if mouse_port_ok {
            self.command(COMMAND_ENABLE_MOUSE);
            if self.to_mouse(DEVICE_SET_DEFAULTS) {
                for rate in [200, 100, 80] {
                    let _ = self.to_mouse(DEVICE_SET_SAMPLE_RATE) && self.to_mouse(rate);
                }
                let id = if self.to_mouse(DEVICE_GET_ID) {
                    self.read_answer()
                } else {
                    None
                };
                self.found.wheel = id == Some(WHEEL_MOUSE_ID);
                self.mouse = MousePackets::new(self.found.wheel);
                self.found.mouse = self.to_mouse(DEVICE_ENABLE_REPORTING);
            }
        }
        self.drain();
        self.found
    }

    /// Hands `event` what the devices sent, at most `budget` bytes' worth; returns how many
    /// bytes it took.
    pub fn poll(&mut self, budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
        let mut taken = 0;
        while taken < budget {
            let status = self.regs.read8(STATUS);
            if status & STATUS_OUTPUT_FULL == 0 {
                break;
            }
            let byte = self.regs.read8(DATA);
            taken += 1;
            if status & STATUS_FROM_MOUSE != 0 {
                self.mouse.byte(byte, event);
            } else {
                self.keyboard.byte(byte, event);
            }
        }
        taken
    }

    /// Shows the lock lights on the keyboard.
    pub fn set_lights(&mut self, lights: super::LockLights) -> bool {
        if !self.found.keyboard {
            return true;
        }
        let byte =
            u8::from(lights.scroll) | (u8::from(lights.num) << 1) | (u8::from(lights.caps) << 2);
        self.to_keyboard(KEYBOARD_SET_LIGHTS) && self.to_keyboard(byte)
    }
}

/// Scan code set 1 (with the `E0` prefix) to Linux key codes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Set1Decoder {
    extended: bool,
    pause: u8,
}

fn extended_key(code: u8) -> u16 {
    match code {
        0x1c => 96,
        0x1d => 97,
        0x35 => 98,
        0x37 => 99,
        0x38 => 100,
        0x47 => 102,
        0x48 => 103,
        0x49 => 104,
        0x4b => 105,
        0x4d => 106,
        0x4f => 107,
        0x50 => 108,
        0x51 => 109,
        0x52 => 110,
        0x53 => 111,
        0x5b => 125,
        0x5c => 126,
        0x5d => 127,
        _ => 0,
    }
}

impl Set1Decoder {
    pub fn byte(&mut self, byte: u8, event: &mut dyn FnMut(RawEvent)) {
        if self.pause > 0 {
            self.pause -= 1;
            if self.pause == 0 {
                emit_key(119, true, event);
                emit_key(119, false, event);
            }
            return;
        }
        match byte {
            0xe0 => {
                self.extended = true;
                return;
            }
            0xe1 => {
                self.pause = 5;
                return;
            }
            _ => {}
        }
        let pressed = byte & 0x80 == 0;
        let code = byte & 0x7f;
        let key = if core::mem::take(&mut self.extended) {
            if code == 0x2a || code == 0x36 {
                return;
            }
            extended_key(code)
        } else {
            u16::from(code)
        };
        if key != 0 {
            emit_key(key, pressed, event);
        }
    }
}

fn emit_key(code: u16, pressed: bool, event: &mut dyn FnMut(RawEvent)) {
    event(RawEvent {
        kind: event_type::KEY,
        code,
        value: i32::from(pressed),
    });
    event(RawEvent {
        kind: event_type::SYN,
        code: SYN_REPORT,
        value: 0,
    });
}

/// PS/2 mouse packets: three bytes, four with a wheel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MousePackets {
    bytes: [u8; 4],
    filled: usize,
    wheel: bool,
    buttons: u8,
}

const ALWAYS_SET: u8 = 1 << 3;
const X_NEGATIVE: u8 = 1 << 4;
const Y_NEGATIVE: u8 = 1 << 5;
const OVERFLOW: u8 = 0b11 << 6;

impl MousePackets {
    pub fn new(wheel: bool) -> Self {
        Self {
            wheel,
            ..Self::default()
        }
    }

    pub fn byte(&mut self, byte: u8, event: &mut dyn FnMut(RawEvent)) {
        if self.filled == 0 && byte & ALWAYS_SET == 0 {
            return;
        }
        self.bytes[self.filled] = byte;
        self.filled += 1;
        let length = if self.wheel { 4 } else { 3 };
        if self.filled < length {
            return;
        }
        self.filled = 0;
        let [flags, x, y, z] = self.bytes;
        let buttons = flags & 0b111;
        let order = [button::LEFT, button::RIGHT, button::MIDDLE];
        for (bit, &code) in order.iter().enumerate() {
            let mask = 1 << bit;
            if self.buttons & mask != buttons & mask {
                event(RawEvent {
                    kind: event_type::KEY,
                    code,
                    value: i32::from(buttons & mask != 0),
                });
            }
        }
        self.buttons = buttons;
        if flags & OVERFLOW == 0 {
            let dx = i32::from(x) - if flags & X_NEGATIVE != 0 { 256 } else { 0 };
            let dy = i32::from(y) - if flags & Y_NEGATIVE != 0 { 256 } else { 0 };
            let dz = if self.wheel {
                i32::from((z << 4) as i8 >> 4)
            } else {
                0
            };
            for (code, value) in [
                (relative::X, dx),
                (relative::Y, -dy),
                (relative::WHEEL, -dz),
            ] {
                if value != 0 {
                    event(RawEvent {
                        kind: event_type::REL,
                        code,
                        value,
                    });
                }
            }
        }
        event(RawEvent {
            kind: event_type::SYN,
            code: SYN_REPORT,
            value: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Vec<RawEvent> {
        let mut decoder = Set1Decoder::default();
        let mut events = Vec::new();
        for &byte in bytes {
            decoder.byte(byte, &mut |event| events.push(event));
        }
        events
            .into_iter()
            .filter(|event| event.kind == event_type::KEY)
            .collect()
    }

    #[test]
    fn set_1_codes_are_linux_key_codes_and_e0_ones_are_mapped() {
        let events = decode(&[0x1e, 0x9e, 0xe0, 0x48, 0xe0, 0xc8]);
        let codes: Vec<_> = events
            .iter()
            .map(|event| (event.code, event.value))
            .collect();
        assert_eq!(codes, vec![(30, 1), (30, 0), (103, 1), (103, 0)]);
    }

    #[test]
    fn the_fake_shifts_of_extended_keys_are_dropped() {
        let events = decode(&[0xe0, 0x2a, 0xe0, 0x52]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].code, 110);
    }

    #[test]
    fn pause_is_one_press_and_release() {
        let events = decode(&[0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].code, 119);
    }

    #[test]
    fn a_wheel_packet_moves_up_and_scrolls() {
        let mut mouse = MousePackets::new(true);
        let mut events = Vec::new();
        for byte in [0x09, 4, 2, 0x0f] {
            mouse.byte(byte, &mut |event| events.push(event));
        }
        assert_eq!(events[0].code, button::LEFT);
        assert_eq!((events[1].code, events[1].value), (relative::X, 4));
        assert_eq!((events[2].code, events[2].value), (relative::Y, -2));
        assert_eq!((events[3].code, events[3].value), (relative::WHEEL, 1));
    }

    #[test]
    fn a_byte_without_bit_3_does_not_start_a_packet() {
        let mut mouse = MousePackets::new(false);
        let mut events = Vec::new();
        for byte in [0x00, 0x08, 1, 1] {
            mouse.byte(byte, &mut |event| events.push(event));
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == event_type::REL)
                .count(),
            2
        );
    }
}
