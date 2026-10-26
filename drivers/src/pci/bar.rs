use super::{Address, BAR0, COMMAND, COMMAND_IO, COMMAND_MEMORY, ConfigSpace};

/// What a base address register decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    Memory32,
    /// A 64-bit memory BAR, which takes its slot and the next.
    Memory64,
    Io,
}

/// One implemented base address register: the window firmware assigned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    /// Its slot, 0 to 5.
    pub index: u8,
    pub kind: BarKind,
    pub prefetchable: bool,
    /// The address firmware assigned (I/O port or physical memory address).
    pub address: u64,
    /// Bytes it decodes, a power of two.
    pub size: u64,
}

const IO_FLAGS: u32 = 0b11;
const MEMORY_FLAGS: u32 = 0b1111;
const MEMORY_TYPE_64: u32 = 0b10 << 1;
const PREFETCHABLE: u32 = 1 << 3;

/// The BARs of the function at `address`, by slot: 6 for a type 0 header, 2 for a type 1
/// (bridge) header, none for others (`header_type`'s low 7 bits). Each is sized the standard
/// way: all ones written, the mask read back, the original value restored, with I/O and memory
/// decoding off meanwhile and the command register restored at the end (status bits, which a
/// written one would clear, are written as zeros). A 64-bit BAR fills its slot and leaves the
/// next `None`; an unimplemented BAR is `None`. Firmware's addresses are kept, never changed.
pub fn bars<C: ConfigSpace + ?Sized>(
    config: &C,
    address: Address,
    header_type: u8,
) -> [Option<Bar>; 6] {
    let mut bars = [None; 6];
    let count = match header_type & 0x7f {
        0 => 6,
        1 => 2,
        _ => return bars,
    };
    let command = config.read16(address, COMMAND);
    config.write32(
        address,
        COMMAND,
        u32::from(command & !(COMMAND_IO | COMMAND_MEMORY)),
    );
    let mut index = 0;
    while index < count {
        let (bar, slots) = size_bar(config, address, index, count);
        bars[usize::from(index)] = bar;
        index += slots;
    }
    config.write32(address, COMMAND, u32::from(command));
    bars
}

fn probe(config: &(impl ConfigSpace + ?Sized), address: Address, offset: u16) -> (u32, u32) {
    let original = config.read32(address, offset);
    config.write32(address, offset, u32::MAX);
    let mask = config.read32(address, offset);
    config.write32(address, offset, original);
    (original, mask)
}

fn size_bar(
    config: &(impl ConfigSpace + ?Sized),
    address: Address,
    index: u8,
    count: u8,
) -> (Option<Bar>, u8) {
    let offset = BAR0 + 4 * u16::from(index);
    let (original, mask) = probe(config, address, offset);
    if original & 1 != 0 {
        let mask = mask & !IO_FLAGS;
        let bar = (mask != 0).then(|| Bar {
            index,
            kind: BarKind::Io,
            prefetchable: false,
            address: u64::from(original & !IO_FLAGS),
            size: u64::from((!(mask | 0xffff_0000)).wrapping_add(1)),
        });
        return (bar, 1);
    }
    let prefetchable = original & PREFETCHABLE != 0;
    if original & (0b11 << 1) == MEMORY_TYPE_64 {
        if index + 1 >= count {
            return (None, 1);
        }
        let (original_high, mask_high) = probe(config, address, offset + 4);
        let mask = u64::from(mask_high) << 32 | u64::from(mask & !MEMORY_FLAGS);
        let bar = (mask != 0).then(|| Bar {
            index,
            kind: BarKind::Memory64,
            prefetchable,
            address: u64::from(original_high) << 32 | u64::from(original & !MEMORY_FLAGS),
            size: (!mask).wrapping_add(1),
        });
        return (bar, 2);
    }
    let mask = mask & !MEMORY_FLAGS;
    let bar = (mask != 0).then(|| Bar {
        index,
        kind: BarKind::Memory32,
        prefetchable,
        address: u64::from(original & !MEMORY_FLAGS),
        size: u64::from((!mask).wrapping_add(1)),
    });
    (bar, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci::fake::FakeConfig;

    fn at(device: u8) -> Address {
        Address::new(0, device, 0).unwrap()
    }

    fn device(header_type: u8) -> FakeConfig {
        let config = FakeConfig::default();
        config.function(at(1), (0x1af4, 0x1044), (0xff, 0), header_type);
        config.set(at(1), COMMAND, 0x0010_0007);
        config
    }

    #[test]
    fn sizes_32_bit_64_bit_io_and_unimplemented_bars() {
        let config = device(0);
        config.register(at(1), 0x10, 0xfebd_1000, 0xffff_f000);
        config.register(at(1), 0x14, 0xc041, 0x0000_ffe0);
        config.register(at(1), 0x18, 0, 0);
        config.register(at(1), 0x20, 0x4000 | 0xc, 0xffff_c000);
        config.register(at(1), 0x24, 0x8, u32::MAX);
        let bars = bars(&config, at(1), 0x80);
        assert_eq!(
            bars,
            [
                Some(Bar {
                    index: 0,
                    kind: BarKind::Memory32,
                    prefetchable: false,
                    address: 0xfebd_1000,
                    size: 0x1000,
                }),
                Some(Bar {
                    index: 1,
                    kind: BarKind::Io,
                    prefetchable: false,
                    address: 0xc040,
                    size: 0x20,
                }),
                None,
                None,
                Some(Bar {
                    index: 4,
                    kind: BarKind::Memory64,
                    prefetchable: true,
                    address: 0x8_0000_4000,
                    size: 0x4000,
                }),
                None,
            ]
        );
        assert_eq!(config.get(at(1), 0x10), 0xfebd_1000);
        assert_eq!(config.get(at(1), 0x14), 0xc041);
        assert_eq!(config.get(at(1), 0x20), 0x400c);
        assert_eq!(config.get(at(1), 0x24), 0x8);
    }

    #[test]
    fn decoding_is_off_while_sizing_and_the_command_comes_back_with_its_status() {
        let config = device(0);
        config.register(at(1), 0x10, 0xfebd_1000, 0xffff_f000);
        bars(&config, at(1), 0);
        let writes = config.writes.borrow();
        let mut command = None;
        for &(_, offset, value) in writes.iter() {
            if offset == COMMAND {
                assert_eq!(value >> 16, 0);
                command = Some(value);
            } else if value == u32::MAX {
                let command = command.unwrap();
                assert_eq!(command & u32::from(COMMAND_IO | COMMAND_MEMORY), 0);
            }
        }
        assert_eq!(writes.first().map(|w| (w.1, w.2)), Some((COMMAND, 0x4)));
        assert_eq!(writes.last().map(|w| (w.1, w.2)), Some((COMMAND, 0x7)));
        drop(writes);
        assert_eq!(config.get(at(1), COMMAND), 0x0010_0007);
    }

    #[test]
    fn a_bridge_has_two_bars_and_other_headers_none() {
        let config = device(1);
        config.register(at(1), 0x10, 0xfe00_0000, 0xfff0_0000);
        config.register(at(1), 0x18, 0x0002_0100, u32::MAX);
        let found = bars(&config, at(1), 1);
        assert_eq!(found[0].map(|bar| bar.size), Some(0x10_0000));
        assert!(found[1..].iter().all(Option::is_none));
        assert!(config.writes.borrow().iter().all(|w| w.1 < 0x18));
        let other = device(2);
        assert_eq!(bars(&other, at(1), 2), [None; 6]);
        assert!(other.writes.borrow().is_empty());
    }

    #[test]
    fn a_64_bit_bar_in_the_last_slot_is_not_read_past_the_header() {
        let config = device(0);
        config.register(at(1), 0x24, 0x4, 0xffff_f000);
        let found = bars(&config, at(1), 0);
        assert_eq!(found[5], None);
        assert!(config.writes.borrow().iter().all(|w| w.1 <= 0x24));
    }
}
