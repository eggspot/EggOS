//! Devices the ACPI namespace declares (D252): every device of the DSDT's static part with a
//! `_HID`, its memory ranges from a `_CRS` named as data, as [`Identity::Acpi`] nodes for drivers
//! that match an `AcpiHid`. A device on an I2C bus (D462) also carries its target address and
//! its controller, found by the path its connection names: the controller's own registers when
//! its `_CRS` names them, or the PCI function its `_ADR` names on the root bus.

use super::{DeviceNode, DeviceTable, DmaCoherence, Identity, MAX_REGIONS};
use alloc::vec::Vec;
use eggos_hal::PhysicalRange;
use eggos_hal::aml;

/// Bytes of an ACPI id and path kept in a device's identity.
pub const ID_BYTES: usize = 16;
pub const PATH_BYTES: usize = 48;

/// Where an I2C device's controller is (D462).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cController {
    /// The registers the controller's own `_CRS` names.
    Registers(PhysicalRange),
    /// A PCI function on the root bus, by its `_ADR`: device and function.
    PciSlot { device: u8, function: u8 },
}

/// A device's place on an I2C bus, from its `I2cSerialBus` connection (D462).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I2cTarget {
    pub address: u16,
    pub ten_bit: bool,
    pub speed_hz: u32,
    /// The HID over I2C descriptor's register its `_DSM` returns, as far as it was read.
    pub descriptor_register: Option<u16>,
    pub controller: I2cController,
}

#[derive(Debug, Clone, Copy)]
struct Declared {
    path: aml::Path,
    registers: Option<PhysicalRange>,
    address: Option<u64>,
}

fn first_registers(resources: Option<&[u8]>) -> Option<PhysicalRange> {
    let mut first = None;
    aml::memory_resources(resources?, &mut |start, length| {
        if first.is_none() && length != 0 {
            first = Some(PhysicalRange::from_start_length(start, length));
        }
    });
    first
}

fn i2c_target(device: &aml::Device<'_>, declared: &[Declared]) -> Option<I2cTarget> {
    let connection = device.i2c?;
    let controller = declared
        .iter()
        .find(|candidate| aml::names_path(connection.controller, &candidate.path))?;
    let controller = match (controller.registers, controller.address) {
        (Some(registers), _) => I2cController::Registers(registers),
        (None, Some(address)) => I2cController::PciSlot {
            device: (address >> 16) as u8,
            function: (address & 0xffff) as u8,
        },
        (None, None) => return None,
    };
    Some(I2cTarget {
        address: connection.address,
        ten_bit: connection.ten_bit,
        speed_hz: connection.speed_hz,
        descriptor_register: device.hid_descriptor_register,
        controller,
    })
}

pub(super) fn discover(dsdt: &[u8], table: &mut DeviceTable, present: &dyn Fn(&[u8]) -> bool) {
    let Some(definitions) = dsdt.get(eggos_hal::acpi::TABLE_HEADER_LENGTH..) else {
        return;
    };
    let mut declared = Vec::new();
    aml::devices(definitions, &mut |device| {
        if declared.try_reserve(1).is_ok() {
            declared.push(Declared {
                path: device.path,
                registers: first_registers(device.resources),
                address: device.address,
            });
        }
    });
    aml::devices(definitions, &mut |device| {
        let Some(hid) = device.hid else {
            return;
        };
        let mut id = [0u8; ID_BYTES];
        let hid_bytes = hid.as_bytes();
        id[..hid_bytes.len().min(ID_BYTES)]
            .copy_from_slice(&hid_bytes[..hid_bytes.len().min(ID_BYTES)]);
        let mut compatible = [0u8; ID_BYTES];
        if let Some(cid) = device.cid {
            let bytes = cid.as_bytes();
            compatible[..bytes.len().min(ID_BYTES)]
                .copy_from_slice(&bytes[..bytes.len().min(ID_BYTES)]);
        }
        let mut path = [0u8; PATH_BYTES];
        device.path.write(&mut path);
        if !present(text(&path)) {
            return;
        }
        let mut node = DeviceNode::new(Identity::Acpi {
            hid: id,
            cid: compatible,
            path,
            i2c: i2c_target(&device, &declared),
        });
        node.dma = DmaCoherence::Coherent;
        let mut slot = 0;
        if let Some(resources) = device.resources {
            aml::memory_resources(resources, &mut |start, length| {
                if slot < MAX_REGIONS && length != 0 {
                    node.regions[slot] = PhysicalRange::from_start_length(start, length);
                    slot += 1;
                }
            });
        }
        table.add(node);
    });
}

/// `bytes` up to their first NUL.
pub fn text(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    &bytes[..end]
}

/// What an ACPI device is to a person, by its `_HID`.
pub fn class(hid: &[u8]) -> &'static str {
    match text(hid) {
        b"PNP0303" | b"PNP030B" | b"PNP0F13" | b"PNP0C50" | b"ACPI0C50" => "input",
        b"PNP0A03" | b"PNP0A08" | b"PNP0A05" | b"PNP0A06" => "bus",
        b"PNP0501" => "serial",
        b"PNP0B00" | b"PNP0B01" | b"PNP0B02" => "clock",
        b"PNP0103" | b"PNP0100" => "clock",
        b"PNP0000" | b"PNP0003" => "interrupt-controller",
        _ => "other",
    }
}
