//! Reading resource templates (ACPI 6.5, 6.4), the buffers `_CRS` returns.

use alloc::vec::Vec;

const SMALL_ITEM_LENGTH_MASK: u8 = 0x07;
const SMALL_ITEM_TYPE_SHIFT: u8 = 3;
const SMALL_ITEM_TYPE_MASK: u8 = 0x0f;
const LARGE_ITEM: u8 = 0x80;
const IO_PORT: u8 = 0x08;
const FIXED_IO_PORT: u8 = 0x09;
const END_TAG: u8 = 0x0f;
const FIXED_IO_ADDRESS_MASK: u16 = 0x03ff;

/// The base port of each I/O port and fixed-location I/O port descriptor in `template`, in
/// order (6.4.2.5 and 6.4.2.6): an embedded controller's data port, then its command port.
pub fn io_ports(template: &[u8]) -> Vec<u16> {
    let mut ports = Vec::new();
    let mut at = 0;
    while let Some(&tag) = template.get(at) {
        if tag & LARGE_ITEM != 0 {
            let Some(length) = template.get(at + 1..at + 3) else {
                break;
            };
            at += 3 + usize::from(u16::from_le_bytes([length[0], length[1]]));
            continue;
        }
        let length = usize::from(tag & SMALL_ITEM_LENGTH_MASK);
        let Some(body) = template.get(at + 1..at + 1 + length) else {
            break;
        };
        match tag >> SMALL_ITEM_TYPE_SHIFT & SMALL_ITEM_TYPE_MASK {
            IO_PORT if body.len() >= 3 => ports.push(u16::from_le_bytes([body[1], body[2]])),
            FIXED_IO_PORT if body.len() >= 2 => {
                ports.push(u16::from_le_bytes([body[0], body[1]]) & FIXED_IO_ADDRESS_MASK);
            }
            END_TAG => break,
            _ => {}
        }
        at += 1 + length;
    }
    ports
}
