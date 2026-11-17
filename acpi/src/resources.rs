//! Reading resource templates (ACPI 6.5, 6.4), the buffers `_CRS` returns.

use alloc::vec::Vec;

const SMALL_ITEM_LENGTH_MASK: u8 = 0x07;
const SMALL_ITEM_TYPE_SHIFT: u8 = 3;
const SMALL_ITEM_TYPE_MASK: u8 = 0x0f;
const LARGE_ITEM: u8 = 0x80;
const IRQ: u8 = 0x04;
const EXTENDED_INTERRUPT: u8 = 0x89;
const IRQ_FLAGS_EDGE: u8 = 1;
const IRQ_FLAGS_ACTIVE_LOW: u8 = 1 << 3;
const EXTENDED_FLAGS_EDGE: u8 = 1 << 1;
const EXTENDED_FLAGS_ACTIVE_LOW: u8 = 1 << 2;
const IO_PORT: u8 = 0x08;
const FIXED_IO_PORT: u8 = 0x09;
const END_TAG: u8 = 0x0f;
const FIXED_IO_ADDRESS_MASK: u16 = 0x03ff;

/// The base port of each I/O port and fixed-location I/O port descriptor in `template`, in
/// order (6.4.2.5 and 6.4.2.6): an embedded controller's data port, then its command port.
pub fn io_ports(template: &[u8]) -> Vec<u16> {
    io_ranges(template).into_iter().map(|(port, _)| port).collect()
}

/// One interrupt a resource template names (6.4.2.1 and 6.4.3.6): its number (an ISA IRQ or a
/// global system interrupt), and how it is signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interrupt {
    pub number: u32,
    pub edge: bool,
    pub active_low: bool,
}

/// The interrupts of `template`'s IRQ and extended interrupt descriptors, in order (D621): what
/// a PCI interrupt link's `_CRS` says it is routed to. An IRQ descriptor without its flags byte
/// is edge-triggered and active-high, as the specification says.
pub fn interrupts(template: &[u8]) -> Vec<Interrupt> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(&tag) = template.get(at) {
        if tag & LARGE_ITEM != 0 {
            let Some(length) = template.get(at + 1..at + 3) else {
                break;
            };
            let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
            let Some(body) = template.get(at + 3..at + 3 + length) else {
                break;
            };
            if tag == EXTENDED_INTERRUPT && body.len() >= 2 {
                let flags = body[0];
                let count = usize::from(body[1]);
                for index in 0..count {
                    let Some(number) = body.get(2 + index * 4..6 + index * 4) else {
                        break;
                    };
                    found.push(Interrupt {
                        number: u32::from_le_bytes([number[0], number[1], number[2], number[3]]),
                        edge: flags & EXTENDED_FLAGS_EDGE != 0,
                        active_low: flags & EXTENDED_FLAGS_ACTIVE_LOW != 0,
                    });
                }
            }
            at += 3 + length;
            continue;
        }
        let length = usize::from(tag & SMALL_ITEM_LENGTH_MASK);
        let Some(body) = template.get(at + 1..at + 1 + length) else {
            break;
        };
        match tag >> SMALL_ITEM_TYPE_SHIFT & SMALL_ITEM_TYPE_MASK {
            IRQ if body.len() >= 2 => {
                let mask = u16::from_le_bytes([body[0], body[1]]);
                let flags = body.get(2).copied().unwrap_or(IRQ_FLAGS_EDGE);
                for number in (0..16).filter(|bit| mask & (1 << bit) != 0) {
                    found.push(Interrupt {
                        number,
                        edge: flags & IRQ_FLAGS_EDGE != 0,
                        active_low: flags & IRQ_FLAGS_ACTIVE_LOW != 0,
                    });
                }
            }
            END_TAG => break,
            _ => {}
        }
        at += 1 + length;
    }
    found
}

/// Each I/O port and fixed-location I/O port descriptor's base port and length in bytes, in
/// order (6.4.2.5 and 6.4.2.6): a GPE block device's register block (D556).
pub fn io_ranges(template: &[u8]) -> Vec<(u16, u8)> {
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
            IO_PORT if body.len() >= 7 => {
                ports.push((u16::from_le_bytes([body[1], body[2]]), body[6]));
            }
            FIXED_IO_PORT if body.len() >= 3 => {
                ports.push((
                    u16::from_le_bytes([body[0], body[1]]) & FIXED_IO_ADDRESS_MASK,
                    body[2],
                ));
            }
            END_TAG => break,
            _ => {}
        }
        at += 1 + length;
    }
    ports
}
