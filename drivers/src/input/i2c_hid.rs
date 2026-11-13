//! HID over I2C (Microsoft's HID over I2C Protocol Specification 1.0; D462): a device's
//! 30-byte HID descriptor at the register firmware names, its report descriptor
//! ([`super::hid_report`]), and its commands through the command and data registers. Sensors
//! are polled: each report that carries an accelerometer's axes is asked for with
//! `GET_REPORT` and decoded as a USB HID sensor's would be, so the screen turns (D435) on the
//! tablets and convertibles whose accelerometer sits on I2C. Devices that only send reports
//! when their interrupt line says so (touchpads, keyboards) wait for GPIO interrupts.

use super::hid_report::{Decoder, Layout};
use super::{Capabilities, RawEvent};
use crate::i2c::{I2cBus, I2cError};
use alloc::vec::Vec;

/// The HID descriptor's length.
pub const DESCRIPTOR_BYTES: usize = 30;
/// The registers the HID descriptor is looked for at when firmware names none that answers:
/// the values devices' `_DSM` returns most often.
pub const LIKELY_DESCRIPTOR_REGISTERS: [u16; 3] = [0x0001, 0x0020, 0x0000];
/// The longest report descriptor read.
pub const LARGEST_REPORT_DESCRIPTOR: usize = 4096;
/// The most reports polled.
pub const MOST_POLLED_REPORTS: usize = 4;
const VERSION_1_0: u16 = 0x0100;
const LONGEST_REPORT: usize = 256;
const LENGTH_BYTES: usize = 2;
const EXTENDED_REPORT_ID: u8 = 0x0f;

mod opcode {
    pub const GET_REPORT: u8 = 0x02;
    pub const SET_POWER: u8 = 0x08;
}

mod report_type {
    pub const INPUT: u8 = 1;
}

const POWER_ON: u8 = 0;

/// Why a device could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cHidError {
    /// The bus failed the transfer.
    Bus(I2cError),
    /// No HID descriptor of version 1.0 at any register tried.
    NoDescriptor,
    /// The report descriptor is empty, too long or malformed.
    BadReportDescriptor,
    /// No memory for the report descriptor.
    NoMemory,
}

impl From<I2cError> for I2cHidError {
    fn from(error: I2cError) -> Self {
        Self::Bus(error)
    }
}

impl I2cHidError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Bus(error) => error.description(),
            Self::NoDescriptor => "no HID over I2C descriptor",
            Self::BadReportDescriptor => "a malformed HID report descriptor",
            Self::NoMemory => "no memory for the HID report descriptor",
        }
    }
}

/// A device's HID descriptor: where its registers are and how long its reports may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HidDescriptor {
    pub report_descriptor_length: u16,
    pub report_descriptor_register: u16,
    pub input_register: u16,
    pub max_input_length: u16,
    pub output_register: u16,
    pub max_output_length: u16,
    pub command_register: u16,
    pub data_register: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}

impl HidDescriptor {
    /// The descriptor `bytes` hold; `None` unless it says it is 30 bytes of version 1.0.
    pub fn parse(bytes: &[u8; DESCRIPTOR_BYTES]) -> Option<Self> {
        let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        if usize::from(word(0)) != DESCRIPTOR_BYTES || word(2) != VERSION_1_0 {
            return None;
        }
        Some(Self {
            report_descriptor_length: word(4),
            report_descriptor_register: word(6),
            input_register: word(8),
            max_input_length: word(10),
            output_register: word(12),
            max_output_length: word(14),
            command_register: word(16),
            data_register: word(18),
            vendor: word(20),
            product: word(22),
            version: word(24),
        })
    }
}

fn command(
    descriptor: &HidDescriptor,
    kind: u8,
    report_id: u8,
    opcode: u8,
    with_data: bool,
    out: &mut [u8; 8],
) -> usize {
    let [low, high] = descriptor.command_register.to_le_bytes();
    out[0] = low;
    out[1] = high;
    let mut length = 4;
    match report_id < EXTENDED_REPORT_ID {
        true => out[2] = (kind << 4) | report_id,
        false => {
            out[2] = (kind << 4) | EXTENDED_REPORT_ID;
            out[4] = report_id;
            length = 5;
        }
    }
    out[3] = opcode;
    if with_data {
        let [low, high] = descriptor.data_register.to_le_bytes();
        out[length] = low;
        out[length + 1] = high;
        length += 2;
    }
    length
}

/// A HID device on an I2C bus, its sensors polled.
#[derive(Debug)]
pub struct I2cHid {
    address: u16,
    ten_bit: bool,
    descriptor: HidDescriptor,
    layout: Layout,
    decoder: Decoder,
    polled: [u8; MOST_POLLED_REPORTS],
    polled_count: usize,
}

impl I2cHid {
    /// Finds the device at `address`'s HID descriptor (at `register` when firmware names one,
    /// else at [`LIKELY_DESCRIPTOR_REGISTERS`]), reads and parses its report descriptor and
    /// powers it on.
    pub fn start<B: I2cBus>(
        bus: &mut B,
        address: u16,
        ten_bit: bool,
        register: Option<u16>,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, I2cHidError> {
        let descriptor = register
            .into_iter()
            .chain(LIKELY_DESCRIPTOR_REGISTERS)
            .find_map(|register| {
                let mut bytes = [0u8; DESCRIPTOR_BYTES];
                bus.transfer(
                    address,
                    ten_bit,
                    &register.to_le_bytes(),
                    &mut bytes,
                    keep_waiting,
                )
                .ok()?;
                HidDescriptor::parse(&bytes)
            })
            .ok_or(I2cHidError::NoDescriptor)?;
        let length = usize::from(descriptor.report_descriptor_length);
        if length == 0 || length > LARGEST_REPORT_DESCRIPTOR {
            return Err(I2cHidError::BadReportDescriptor);
        }
        let mut report_descriptor = Vec::new();
        report_descriptor
            .try_reserve_exact(length)
            .map_err(|_| I2cHidError::NoMemory)?;
        report_descriptor.resize(length, 0);
        bus.transfer(
            address,
            ten_bit,
            &descriptor.report_descriptor_register.to_le_bytes(),
            &mut report_descriptor,
            keep_waiting,
        )?;
        let layout = Layout::parse(&report_descriptor).ok_or(I2cHidError::BadReportDescriptor)?;
        let mut polled = [0u8; MOST_POLLED_REPORTS];
        let polled_count = layout.sensor_reports(&mut polled);
        let mut device = Self {
            address,
            ten_bit,
            descriptor,
            layout,
            decoder: Decoder::default(),
            polled,
            polled_count,
        };
        device.set_power_on(bus, keep_waiting)?;
        Ok(device)
    }

    fn set_power_on<B: I2cBus>(
        &mut self,
        bus: &mut B,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), I2cHidError> {
        let mut bytes = [0u8; 8];
        let length = command(
            &self.descriptor,
            0,
            POWER_ON,
            opcode::SET_POWER,
            false,
            &mut bytes,
        );
        bus.transfer(
            self.address,
            self.ten_bit,
            &bytes[..length],
            &mut [],
            keep_waiting,
        )?;
        Ok(())
    }

    /// Its HID descriptor.
    pub fn descriptor(&self) -> &HidDescriptor {
        &self.descriptor
    }

    /// What it reports.
    pub fn capabilities(&self) -> Capabilities {
        self.layout.capabilities()
    }

    /// Whether it has reports to poll: an accelerometer.
    pub fn has_polled_reports(&self) -> bool {
        self.polled_count > 0
    }

    /// Asks for each polled report with `GET_REPORT` and hands `event` what changed; how many
    /// events it sent.
    pub fn poll<B: I2cBus>(
        &mut self,
        bus: &mut B,
        keep_waiting: &mut dyn FnMut() -> bool,
        event: &mut dyn FnMut(RawEvent),
    ) -> Result<usize, I2cHidError> {
        let mut sent = 0;
        let mut answer = [0u8; LENGTH_BYTES + LONGEST_REPORT];
        let room =
            usize::from(self.descriptor.max_input_length).clamp(LENGTH_BYTES + 1, answer.len());
        for &report_id in &self.polled[..self.polled_count] {
            let mut request = [0u8; 8];
            let length = command(
                &self.descriptor,
                report_type::INPUT,
                report_id,
                opcode::GET_REPORT,
                true,
                &mut request,
            );
            bus.transfer(
                self.address,
                self.ten_bit,
                &request[..length],
                &mut answer[..room],
                keep_waiting,
            )?;
            let told = usize::from(u16::from_le_bytes([answer[0], answer[1]]));
            let end = told.clamp(LENGTH_BYTES, room);
            let report = &answer[LENGTH_BYTES..end];
            if !report.is_empty() {
                sent += self.decoder.report(&self.layout, report, event);
            }
        }
        Ok(sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{absolute, event_type};
    use std::collections::HashMap;

    const ADDRESS: u16 = 0x15;

    const ACCELEROMETER: &[u8] = &[
        0x05, 0x20, 0x09, 0x73, 0xa1, 0x00, 0x85, 0x02, 0x0a, 0x53, 0x04, 0x0a, 0x54, 0x04, 0x0a,
        0x55, 0x04, 0x16, 0x00, 0x80, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x03, 0x81, 0x02, 0xc0,
    ];

    #[derive(Debug, Default)]
    struct FakeBus {
        registers: HashMap<u16, Vec<u8>>,
        writes: Vec<Vec<u8>>,
        answer_at_data: Vec<u8>,
    }

    impl I2cBus for FakeBus {
        fn transfer(
            &mut self,
            address: u16,
            _ten_bit: bool,
            write: &[u8],
            read: &mut [u8],
            _keep_waiting: &mut dyn FnMut() -> bool,
        ) -> Result<(), I2cError> {
            if address != ADDRESS {
                return Err(I2cError::NotAcknowledged(1));
            }
            self.writes.push(write.to_vec());
            if read.is_empty() {
                return Ok(());
            }
            let register = u16::from_le_bytes([write[0], write[1]]);
            let source = match write.len() > 2 {
                true => &self.answer_at_data,
                false => self
                    .registers
                    .get(&register)
                    .ok_or(I2cError::NotAcknowledged(2))?,
            };
            for (place, byte) in read
                .iter_mut()
                .zip(source.iter().chain(core::iter::repeat(&0)))
            {
                *place = *byte;
            }
            Ok(())
        }
    }

    fn descriptor_bytes() -> Vec<u8> {
        let words: [u16; 13] = [
            30,
            VERSION_1_0,
            ACCELEROMETER.len() as u16,
            0x0002,
            0x0003,
            9,
            0x0004,
            0,
            0x0005,
            0x0006,
            0x1234,
            0x5678,
            1,
        ];
        let mut bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }

    fn sensor_at(descriptor_register: u16) -> FakeBus {
        let mut bus = FakeBus::default();
        bus.registers
            .insert(descriptor_register, descriptor_bytes());
        bus.registers.insert(0x0002, ACCELEROMETER.to_vec());
        bus
    }

    fn polls() -> impl FnMut() -> bool {
        || true
    }

    #[test]
    fn the_descriptor_is_found_at_a_likely_register_and_the_device_powered_on() {
        let mut bus = sensor_at(0x0020);
        let device = I2cHid::start(&mut bus, ADDRESS, false, None, &mut polls()).unwrap();
        assert_eq!(device.descriptor().vendor, 0x1234);
        assert_eq!(device.descriptor().command_register, 0x0005);
        assert!(device.has_polled_reports());
        assert_eq!(
            bus.writes.last(),
            Some(&vec![0x05, 0x00, 0x00, opcode::SET_POWER])
        );
    }

    #[test]
    fn the_register_firmware_names_is_tried_first() {
        let mut bus = sensor_at(0x0042);
        I2cHid::start(&mut bus, ADDRESS, false, Some(0x0042), &mut polls()).unwrap();
        assert_eq!(bus.writes[0], vec![0x42, 0x00]);
    }

    #[test]
    fn a_device_without_a_descriptor_is_refused() {
        let mut bus = FakeBus::default();
        let outcome = I2cHid::start(&mut bus, ADDRESS, false, None, &mut polls());
        assert_eq!(outcome.err(), Some(I2cHidError::NoDescriptor));
    }

    #[test]
    fn a_poll_asks_for_the_accelerometer_s_report_and_decodes_its_axes() {
        let mut bus = sensor_at(0x0001);
        let mut device = I2cHid::start(&mut bus, ADDRESS, false, None, &mut polls()).unwrap();
        bus.answer_at_data = vec![9, 0, 0x02, 0x10, 0x00, 0xf0, 0xff, 0x00, 0x40];
        let mut events = Vec::new();
        device
            .poll(&mut bus, &mut polls(), &mut |event| events.push(event))
            .unwrap();
        assert_eq!(
            bus.writes.last(),
            Some(&vec![0x05, 0x00, 0x12, opcode::GET_REPORT, 0x06, 0x00])
        );
        let axes: Vec<(u16, i32)> = events
            .iter()
            .filter(|event| event.kind == event_type::ABS)
            .map(|event| (event.code, event.value))
            .collect();
        assert_eq!(
            axes,
            vec![
                (absolute::ACCELERATION_X, 16),
                (absolute::ACCELERATION_Y, -16),
                (absolute::ACCELERATION_Z, 0x4000),
            ]
        );
    }

    #[test]
    fn report_ids_from_15_on_take_a_byte_of_their_own() {
        let descriptor = HidDescriptor {
            command_register: 0x0005,
            data_register: 0x0006,
            ..HidDescriptor::default()
        };
        let mut out = [0u8; 8];
        let length = command(
            &descriptor,
            report_type::INPUT,
            20,
            opcode::GET_REPORT,
            true,
            &mut out,
        );
        assert_eq!(
            out[..length],
            [0x05, 0x00, 0x1f, opcode::GET_REPORT, 20, 0x06, 0x00]
        );
    }
}
