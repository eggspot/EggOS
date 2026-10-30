//! PCI and PCI Express configuration space: finding functions, their BARs and capabilities.
//!
//! Layouts are from the PCI Local Bus Specification 3.0 (the type 0 and type 1 headers, the
//! capability list) and the PCI Express Base Specification (ECAM). Everything here reaches
//! configuration space through [`ConfigSpace`], so it runs against fake registers in host tests.

mod bar;
pub mod ecam;
#[cfg(test)]
pub(crate) mod fake;
pub mod mcfg;
pub mod msix;

pub use bar::{Bar, BarKind, bars};

/// A function's place on the bus: bus, device (0-31) and function (0-7) numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Address {
    /// The function at `bus`, `device`, `function`; `None` past device 31 or function 7.
    pub const fn new(bus: u8, device: u8, function: u8) -> Option<Self> {
        if device < 32 && function < 8 {
            Some(Self {
                bus,
                device,
                function,
            })
        } else {
            None
        }
    }
}

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}

/// Access to the configuration space of every function on one PCI segment, in aligned 32-bit
/// words (what every configuration mechanism supports).
pub trait ConfigSpace {
    /// The 32-bit word at byte `offset` (a multiple of 4, below 4096) of `address`'s space. A
    /// function that does not exist reads as all ones.
    fn read32(&self, address: Address, offset: u16) -> u32;
    /// Writes the 32-bit word at byte `offset` (a multiple of 4, below 4096) of `address`'s
    /// space.
    fn write32(&self, address: Address, offset: u16, value: u32);

    /// The 16-bit field at byte `offset` (a multiple of 2), read as part of its word.
    fn read16(&self, address: Address, offset: u16) -> u16 {
        (self.read32(address, offset & !3) >> ((offset & 2) * 8)) as u16
    }

    /// The byte at `offset`, read as part of its word.
    fn read8(&self, address: Address, offset: u16) -> u8 {
        (self.read32(address, offset & !3) >> ((offset & 3) * 8)) as u8
    }
}

/// Header register offsets (type 0 and type 1 headers unless noted).
pub const VENDOR_ID: u16 = 0x00;
pub const DEVICE_ID: u16 = 0x02;
pub const COMMAND: u16 = 0x04;
pub const REVISION: u16 = 0x08;
pub const PROG_IF: u16 = 0x09;
pub const SUBCLASS: u16 = 0x0a;
pub const CLASS: u16 = 0x0b;
pub const HEADER_TYPE: u16 = 0x0e;
/// The first base address register; the others follow every 4 bytes.
pub const BAR0: u16 = 0x10;
/// A type 1 (bridge) header's secondary bus number.
pub const SECONDARY_BUS: u16 = 0x19;
pub const INTERRUPT_LINE: u16 = 0x3c;
pub const INTERRUPT_PIN: u16 = 0x3d;

/// [`HEADER_TYPE`] bit: the device has more functions than function 0.
pub const HEADER_TYPE_MULTI_FUNCTION: u8 = 0x80;
/// [`HEADER_TYPE`]'s low 7 bits of a PCI-to-PCI bridge.
pub const HEADER_TYPE_BRIDGE: u8 = 1;

/// [`COMMAND`] bits: decode I/O ports, decode memory, and master the bus (DMA).
pub const COMMAND_IO: u16 = 1 << 0;
pub const COMMAND_MEMORY: u16 = 1 << 1;
pub const COMMAND_BUS_MASTER: u16 = 1 << 2;

/// The header's capabilities pointer (type 0 and type 1 headers).
pub const CAPABILITIES_POINTER: u16 = 0x34;
/// The status register; bit 4 says the function has a capability list.
pub const STATUS: u16 = 0x06;
/// [`STATUS`] bit: the capabilities pointer is valid.
pub const STATUS_CAPABILITY_LIST: u16 = 1 << 4;

const MAX_CAPABILITIES: usize = 48;

/// One entry of a function's capability list: its id and where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// The capability id (0x09: vendor specific, 0x10: PCI Express, 0x11: MSI-X, ...).
    pub id: u8,
    /// Byte offset of the capability in configuration space.
    pub offset: u8,
}

/// The capability list of the function at `address`, in list order: nothing when the status
/// register says it has none. Pointers below the header (0x40) or that would make the walk
/// longer than any list can be end it, so a broken list never loops.
pub fn capabilities<C: ConfigSpace + ?Sized>(
    config: &C,
    address: Address,
) -> impl Iterator<Item = Capability> + '_ {
    let has_list = config.read16(address, STATUS) & STATUS_CAPABILITY_LIST != 0;
    let mut next = if has_list {
        config.read8(address, CAPABILITIES_POINTER) & !3
    } else {
        0
    };
    let mut steps = 0;
    core::iter::from_fn(move || {
        if next < 0x40 || steps == MAX_CAPABILITIES {
            return None;
        }
        steps += 1;
        let offset = next;
        let header = config.read16(address, u16::from(offset));
        next = (header >> 8) as u8 & !3;
        Some(Capability {
            id: header as u8,
            offset,
        })
    })
}

/// What identifies a function, from its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    pub address: Address,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    /// The header layout, [`HEADER_TYPE`] without the multi-function bit.
    pub header_type: u8,
    pub interrupt_pin: u8,
    pub interrupt_line: u8,
}

/// The function at `address`, or `None` when nothing answers there (vendor id all ones).
pub fn read_function<C: ConfigSpace + ?Sized>(config: &C, address: Address) -> Option<Function> {
    let id = config.read32(address, VENDOR_ID);
    if id as u16 == 0xffff {
        return None;
    }
    let class = config.read32(address, REVISION);
    let interrupt = config.read32(address, INTERRUPT_LINE);
    Some(Function {
        address,
        vendor_id: id as u16,
        device_id: (id >> 16) as u16,
        class: (class >> 24) as u8,
        subclass: (class >> 16) as u8,
        prog_if: (class >> 8) as u8,
        revision: class as u8,
        header_type: config.read8(address, HEADER_TYPE) & !HEADER_TYPE_MULTI_FUNCTION,
        interrupt_pin: (interrupt >> 8) as u8,
        interrupt_line: interrupt as u8,
    })
}

/// Calls `found` with every function on buses `start_bus..=end_bus` reachable from
/// `start_bus`, depth first in scan order: devices 0 to 31, functions 1 to 7 only on a device
/// whose function 0 says it has more, and behind a PCI-to-PCI bridge its secondary bus, right
/// after the bridge itself. A secondary bus must be above the bridge's own bus, in the range and
/// not scanned yet, so a broken topology never loops or lists a function twice: each bus is
/// scanned at most once.
pub fn enumerate<C: ConfigSpace + ?Sized>(
    config: &C,
    start_bus: u8,
    end_bus: u8,
    found: &mut dyn FnMut(Function),
) {
    if start_bus > end_bus {
        return;
    }
    let mut stack = [(0u8, 0u8, 0u8); 256];
    stack[0] = (start_bus, 0, 0);
    let mut scanned = [0u64; 4];
    let mut first_scan = |bus: u8| {
        let (word, bit) = (usize::from(bus / 64), bus % 64);
        let first = scanned[word] & 1 << bit == 0;
        scanned[word] |= 1 << bit;
        first
    };
    first_scan(start_bus);
    let mut depth = 1;
    while depth > 0 {
        let (bus, device, function) = stack[depth - 1];
        if device == 32 {
            depth -= 1;
            continue;
        }
        let Some(address) = Address::new(bus, device, function) else {
            depth -= 1;
            continue;
        };
        let present = read_function(config, address);
        let more_functions = function > 0
            || present.is_some()
                && config.read8(address, HEADER_TYPE) & HEADER_TYPE_MULTI_FUNCTION != 0;
        stack[depth - 1] = match (more_functions, function) {
            (true, 0..=6) => (bus, device, function + 1),
            _ => (bus, device + 1, 0),
        };
        let Some(present) = present else {
            continue;
        };
        found(present);
        if present.header_type == HEADER_TYPE_BRIDGE {
            let secondary = config.read8(address, SECONDARY_BUS);
            if secondary > bus
                && secondary <= end_bus
                && depth < stack.len()
                && first_scan(secondary)
            {
                stack[depth] = (secondary, 0, 0);
                depth += 1;
            }
        }
    }
}

/// Lets the function at `address` decode its memory BARs and master the bus (DMA), keeping its
/// other command bits; status bits, which a written one would clear, are written as zeros.
pub fn enable_memory_and_bus_master<C: ConfigSpace + ?Sized>(config: &C, address: Address) {
    let command = config.read16(address, COMMAND);
    config.write32(
        address,
        COMMAND,
        u32::from(command | COMMAND_MEMORY | COMMAND_BUS_MASTER),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake::FakeConfig;

    fn at(bus: u8, device: u8, function: u8) -> Address {
        Address::new(bus, device, function).unwrap()
    }

    fn plain(config: &FakeConfig, address: Address) {
        config.function(address, (0x8086, 0x29c0), (6, 0), 0);
    }

    fn bridge(config: &FakeConfig, address: Address, secondary: u8) {
        config.function(address, (0x1b36, 0x000c), (6, 4), 1);
        config.set(
            address,
            0x18,
            u32::from(secondary) << 8 | u32::from(address.bus),
        );
    }

    fn found(config: &FakeConfig, start: u8, end: u8) -> Vec<Address> {
        let mut addresses = Vec::new();
        enumerate(config, start, end, &mut |function| {
            addresses.push(function.address)
        });
        addresses
    }

    #[test]
    fn addresses_name_bus_device_and_function_within_their_ranges() {
        assert_eq!(at(0, 31, 7).to_string(), "00:1f.7");
        assert_eq!(Address::new(0, 32, 0), None);
        assert_eq!(Address::new(0, 0, 8), None);
    }

    #[test]
    fn reads_a_functions_identity_and_nothing_where_none_answers() {
        let config = FakeConfig::default();
        config.function(at(0, 4, 0), (0x1af4, 0x1044), (0xff, 0), 0x80);
        config.set(at(0, 4, 0), REVISION, 0xff00_1201);
        config.set(at(0, 4, 0), INTERRUPT_LINE, 0x0000_010b);
        let function = read_function(&config, at(0, 4, 0)).unwrap();
        assert_eq!(
            function,
            Function {
                address: at(0, 4, 0),
                vendor_id: 0x1af4,
                device_id: 0x1044,
                class: 0xff,
                subclass: 0,
                prog_if: 0x12,
                revision: 1,
                header_type: 0,
                interrupt_pin: 1,
                interrupt_line: 0x0b,
            }
        );
        assert_eq!(read_function(&config, at(0, 5, 0)), None);
    }

    #[test]
    fn functions_past_0_are_scanned_only_on_multi_function_devices() {
        let config = FakeConfig::default();
        config.function(at(0, 0x1f, 0), (0x8086, 0x2918), (6, 1), 0x80);
        config.function(at(0, 0x1f, 2), (0x8086, 0x2922), (1, 6), 0);
        config.function(at(0, 0x1f, 3), (0x8086, 0x2930), (0xc, 5), 0);
        plain(&config, at(0, 1, 0));
        for function in 1..8 {
            plain(&config, at(0, 1, function));
        }
        assert_eq!(
            found(&config, 0, 255),
            [at(0, 1, 0), at(0, 0x1f, 0), at(0, 0x1f, 2), at(0, 0x1f, 3)]
        );
    }

    #[test]
    fn a_bridges_secondary_bus_comes_right_after_it() {
        let config = FakeConfig::default();
        plain(&config, at(0, 0, 0));
        bridge(&config, at(0, 1, 0), 1);
        plain(&config, at(1, 0, 0));
        bridge(&config, at(1, 3, 0), 2);
        plain(&config, at(2, 5, 0));
        plain(&config, at(0, 2, 0));
        assert_eq!(
            found(&config, 0, 255),
            [
                at(0, 0, 0),
                at(0, 1, 0),
                at(1, 0, 0),
                at(1, 3, 0),
                at(2, 5, 0),
                at(0, 2, 0)
            ]
        );
    }

    #[test]
    fn a_bus_two_bridges_point_at_is_scanned_once() {
        let config = FakeConfig::default();
        bridge(&config, at(0, 1, 0), 1);
        bridge(&config, at(0, 2, 0), 1);
        plain(&config, at(1, 0, 0));
        assert_eq!(
            found(&config, 0, 255),
            [at(0, 1, 0), at(1, 0, 0), at(0, 2, 0)]
        );
    }

    #[test]
    fn bridges_pointing_back_to_themselves_or_out_of_range_are_not_followed() {
        let config = FakeConfig::default();
        bridge(&config, at(0, 1, 0), 1);
        bridge(&config, at(1, 0, 0), 0);
        bridge(&config, at(1, 1, 0), 1);
        bridge(&config, at(0, 2, 0), 0);
        bridge(&config, at(0, 3, 0), 5);
        plain(&config, at(5, 0, 0));
        assert_eq!(
            found(&config, 0, 4),
            [
                at(0, 1, 0),
                at(1, 0, 0),
                at(1, 1, 0),
                at(0, 2, 0),
                at(0, 3, 0)
            ]
        );
        assert_eq!(found(&config, 1, 1), [at(1, 0, 0), at(1, 1, 0)]);
        assert!(found(&config, 2, 1).is_empty());
    }

    #[test]
    fn a_chain_of_bridges_through_every_bus_ends() {
        let config = FakeConfig::default();
        for bus in 0..255u8 {
            bridge(&config, at(bus, 0, 0), bus + 1);
        }
        plain(&config, at(255, 0, 0));
        let addresses = found(&config, 0, 255);
        assert_eq!(addresses.len(), 256);
        assert_eq!(addresses.last(), Some(&at(255, 0, 0)));
    }

    fn with_capabilities(list: &[(u8, u8)], first: u8) -> FakeConfig {
        let config = FakeConfig::default();
        config.function(at(0, 3, 0), (0x1af4, 0x1044), (0xff, 0), 0);
        config.set(
            at(0, 3, 0),
            COMMAND,
            u32::from(STATUS_CAPABILITY_LIST) << 16,
        );
        config.set(at(0, 3, 0), CAPABILITIES_POINTER, u32::from(first));
        for &(offset, id) in list {
            let next = list
                .iter()
                .skip_while(|&&(o, _)| o != offset)
                .nth(1)
                .map_or(0, |&(o, _)| o);
            config.set(
                at(0, 3, 0),
                u16::from(offset),
                u32::from(next) << 8 | u32::from(id),
            );
        }
        config
    }

    #[test]
    fn walks_qemus_virtio_capability_list_in_order() {
        let list = [
            (0x98, 0x11),
            (0x84, 0x09),
            (0x70, 0x09),
            (0x60, 0x09),
            (0x50, 0x09),
            (0x40, 0x09),
        ];
        let config = with_capabilities(&list, 0x98);
        let walked: Vec<_> = capabilities(&config, at(0, 3, 0))
            .map(|c| (c.offset, c.id))
            .collect();
        assert_eq!(walked, list);
    }

    #[test]
    fn no_list_bit_means_no_capabilities() {
        let config = with_capabilities(&[(0x40, 0x09)], 0x40);
        config.set(at(0, 3, 0), COMMAND, 0);
        assert_eq!(capabilities(&config, at(0, 3, 0)).count(), 0);
    }

    #[test]
    fn broken_lists_stop() {
        let config = with_capabilities(&[(0x40, 0x09)], 0x42);
        config.set(at(0, 3, 0), 0x40, 0x4109);
        let walked: Vec<_> = capabilities(&config, at(0, 3, 0))
            .map(|c| c.offset)
            .collect();
        assert_eq!(walked.len(), 48);
        assert!(walked.iter().all(|&offset| offset == 0x40));
        let low = with_capabilities(&[(0x40, 0x09)], 0x40);
        low.set(at(0, 3, 0), 0x40, 0x3c09);
        assert_eq!(capabilities(&low, at(0, 3, 0)).count(), 1);
        let below = with_capabilities(&[], 0x3c);
        assert_eq!(capabilities(&below, at(0, 3, 0)).count(), 0);
    }

    #[test]
    fn memory_and_bus_mastering_go_on_without_clearing_status() {
        let config = FakeConfig::default();
        plain(&config, at(0, 3, 0));
        config.set(at(0, 3, 0), COMMAND, 0x0010_0401);
        enable_memory_and_bus_master(&config, at(0, 3, 0));
        assert_eq!(config.get(at(0, 3, 0), COMMAND), 0x0010_0407);
        assert_eq!(*config.writes.borrow(), [(at(0, 3, 0), COMMAND, 0x407)]);
    }
}
