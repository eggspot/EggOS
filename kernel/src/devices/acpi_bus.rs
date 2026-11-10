//! Devices the ACPI namespace declares (D252): every device of the DSDT's static part with a
//! `_HID`, its memory ranges from a `_CRS` named as data, as [`Identity::Acpi`] nodes for drivers
//! that match an `AcpiHid`.

use super::{DeviceNode, DeviceTable, DmaCoherence, Identity, MAX_REGIONS};
use eggos_hal::PhysicalRange;
use eggos_hal::aml;

/// Bytes of an ACPI id and path kept in a device's identity.
pub const ID_BYTES: usize = 16;
pub const PATH_BYTES: usize = 48;

pub(super) fn discover(dsdt: &[u8], table: &mut DeviceTable) {
    let Some(definitions) = dsdt.get(eggos_hal::acpi::TABLE_HEADER_LENGTH..) else {
        return;
    };
    aml::devices(definitions, &mut |device| {
        let Some(hid) = device.hid else {
            return;
        };
        let mut id = [0u8; ID_BYTES];
        let hid_bytes = hid.as_bytes();
        id[..hid_bytes.len().min(ID_BYTES)].copy_from_slice(&hid_bytes[..hid_bytes.len().min(ID_BYTES)]);
        let mut compatible = [0u8; ID_BYTES];
        if let Some(cid) = device.cid {
            let bytes = cid.as_bytes();
            compatible[..bytes.len().min(ID_BYTES)].copy_from_slice(&bytes[..bytes.len().min(ID_BYTES)]);
        }
        let mut path = [0u8; PATH_BYTES];
        device.path.write(&mut path);
        let mut node = DeviceNode::new(Identity::Acpi {
            hid: id,
            cid: compatible,
            path,
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
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
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
