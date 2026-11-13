//! PCI Express native hot-plug (PCIe base specification 6.7, D567): the downstream ports whose
//! slot can take and lose a card, what changed in a slot, and the memory windows behind a port
//! that the BARs of a card that arrived are given addresses in.

use super::bar::{self, Bar, BarKind};
use super::{Address, ConfigSpace, capabilities};

const EXPRESS_CAPABILITY: u8 = 0x10;
const LINK_STATUS: u16 = 0x12;
const SLOT_CAPABILITIES: u16 = 0x14;
const SLOT_CONTROL: u16 = 0x18;
const SLOT_STATUS: u16 = 0x1a;
const SLOT_HOT_PLUG_CAPABLE: u32 = 1 << 6;
const CONTROL_PRESENCE_CHANGED: u16 = 1 << 3;
const CONTROL_HOT_PLUG_INTERRUPT: u16 = 1 << 5;
const CONTROL_LINK_CHANGED: u16 = 1 << 12;
const STATUS_ATTENTION_BUTTON: u16 = 1 << 0;
const STATUS_PRESENCE_CHANGED: u16 = 1 << 3;
const STATUS_COMMAND_COMPLETED: u16 = 1 << 4;
const STATUS_PRESENT: u16 = 1 << 6;
const STATUS_LINK_CHANGED: u16 = 1 << 8;
const LINK_ACTIVE: u16 = 1 << 13;
const MEMORY_BASE: u16 = 0x20;
const PREFETCHABLE_BASE: u16 = 0x24;
const PREFETCHABLE_BASE_UPPER: u16 = 0x28;
const PREFETCHABLE_LIMIT_UPPER: u16 = 0x2c;
const WINDOW_GRANULE: u64 = 1 << 20;

/// A downstream port whose slot takes cards while the machine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotPlugPort {
    /// The port (a PCI-to-PCI bridge).
    pub address: Address,
    /// Where its PCI Express capability starts.
    pub express: u8,
    /// The bus behind it.
    pub secondary_bus: u8,
}

/// What [`HotPlugPort::take_events`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotEvents {
    /// A card is in the slot now.
    pub present: bool,
    /// The link to it is up now.
    pub link_up: bool,
    /// Presence, the link or the attention button changed since the last look.
    pub changed: bool,
}

impl HotPlugPort {
    /// The port at `address` if it is a bridge with a hot-plug capable slot.
    pub fn find<C: ConfigSpace + ?Sized>(config: &C, address: Address) -> Option<Self> {
        let express = capabilities(config, address)
            .find(|capability| capability.id == EXPRESS_CAPABILITY)?
            .offset;
        let slot = config.read32(address, u16::from(express) + SLOT_CAPABILITIES);
        if slot & SLOT_HOT_PLUG_CAPABLE == 0 {
            return None;
        }
        Some(Self {
            address,
            express,
            secondary_bus: config.read8(address, super::SECONDARY_BUS),
        })
    }

    fn at(&self, register: u16) -> u16 {
        u16::from(self.express) + register
    }

    /// Has the slot report presence and link changes (polled; the interrupt stays off until
    /// the kernel routes the port's MSI).
    pub fn enable<C: ConfigSpace + ?Sized>(&self, config: &C) {
        let control = config.read16(self.address, self.at(SLOT_CONTROL));
        let wanted = (control | CONTROL_PRESENCE_CHANGED | CONTROL_LINK_CHANGED)
            & !CONTROL_HOT_PLUG_INTERRUPT;
        write16(config, self.address, self.at(SLOT_CONTROL), wanted);
    }

    /// What the slot says now, the change bits cleared (they clear by writing ones).
    pub fn take_events<C: ConfigSpace + ?Sized>(&self, config: &C) -> SlotEvents {
        let status = config.read16(self.address, self.at(SLOT_STATUS));
        let changes = status
            & (STATUS_PRESENCE_CHANGED | STATUS_LINK_CHANGED | STATUS_ATTENTION_BUTTON | STATUS_COMMAND_COMPLETED);
        if changes != 0 {
            write16(config, self.address, self.at(SLOT_STATUS), changes);
        }
        let link = config.read16(self.address, self.at(LINK_STATUS));
        SlotEvents {
            present: status & STATUS_PRESENT != 0,
            link_up: link & LINK_ACTIVE != 0,
            changed: changes & !STATUS_COMMAND_COMPLETED != 0,
        }
    }

    /// The memory windows the port forwards to its bus: the non-prefetchable one and the
    /// prefetchable one, each `start..end`; empty ones are `None`.
    pub fn windows<C: ConfigSpace + ?Sized>(&self, config: &C) -> [Option<(u64, u64)>; 2] {
        let memory = config.read32(self.address, MEMORY_BASE);
        let base = u64::from(memory & 0xfff0) << 16;
        let limit = (u64::from(memory >> 20) << 20) + WINDOW_GRANULE;
        let prefetchable = config.read32(self.address, PREFETCHABLE_BASE);
        let upper_base = u64::from(config.read32(self.address, PREFETCHABLE_BASE_UPPER));
        let upper_limit = u64::from(config.read32(self.address, PREFETCHABLE_LIMIT_UPPER));
        let wide = prefetchable & 0xf == 1;
        let prefetch_base = (u64::from(prefetchable & 0xfff0) << 16)
            | if wide { upper_base << 32 } else { 0 };
        let prefetch_limit = ((u64::from(prefetchable >> 20) << 20)
            | if wide { upper_limit << 32 } else { 0 })
            + WINDOW_GRANULE;
        let window = |start: u64, end: u64| (start < end).then_some((start, end));
        [window(base, limit), window(prefetch_base, prefetch_limit)]
    }
}

fn write16<C: ConfigSpace + ?Sized>(config: &C, address: Address, offset: u16, value: u16) {
    let word = config.read32(address, offset & !3);
    let shift = (offset & 2) * 8;
    let merged = (word & !(0xffff << shift)) | u32::from(value) << shift;
    config.write32(address, offset & !3, merged);
}

/// Hands out addresses in a port's windows to the BARs of the cards behind it: a bump
/// allocator, each BAR aligned to its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowAllocator {
    windows: [Option<(u64, u64)>; 2],
    next: [u64; 2],
}

impl WindowAllocator {
    /// An allocator over `windows` ([`HotPlugPort::windows`]), from their starts.
    pub fn new(windows: [Option<(u64, u64)>; 2]) -> Self {
        Self {
            windows,
            next: windows.map(|window| window.map_or(0, |(start, _)| start)),
        }
    }

    /// Keeps the addresses `start..end` (a BAR already placed) from being handed out again.
    pub fn note_used(&mut self, start: u64, end: u64) {
        for (index, window) in self.windows.iter().enumerate() {
            if let Some((window_start, window_end)) = *window
                && window_start <= start
                && end <= window_end
                && self.next[index] < end
            {
                self.next[index] = end;
            }
        }
    }

    /// An address for `bar`: in the prefetchable window for a prefetchable BAR when there is
    /// one, the other otherwise; a 32-bit BAR only below 4 GiB. `None` when nothing fits.
    pub fn place(&mut self, bar: &Bar) -> Option<u64> {
        if bar.kind == BarKind::Io || bar.size == 0 {
            return None;
        }
        let order: [usize; 2] = match bar.prefetchable {
            true => [1, 0],
            false => [0, 1],
        };
        for index in order {
            let Some((_, end)) = self.windows[index] else {
                continue;
            };
            if index == 1 && !bar.prefetchable {
                continue;
            }
            let start = self.next[index].checked_add(bar.size - 1)? & !(bar.size - 1);
            let last = start.checked_add(bar.size)?;
            if last > end || (bar.kind == BarKind::Memory32 && last > 1 << 32) {
                continue;
            }
            self.next[index] = last;
            return Some(start);
        }
        None
    }

    /// Gives every memory BAR of the function at `address` an address, writing it, and turns
    /// its memory decoding and bus mastering on: the BARs with their new addresses.
    pub fn assign_function<C: ConfigSpace + ?Sized>(
        &mut self,
        config: &C,
        address: Address,
        header_type: u8,
    ) -> [Option<Bar>; 6] {
        let mut bars = bar::bars(config, address, header_type);
        for slot in bars.iter_mut() {
            let Some(found) = slot else {
                continue;
            };
            match self.place(found) {
                Some(placed) => {
                    bar::assign(config, address, found, placed);
                    found.address = placed;
                }
                None => *slot = None,
            }
        }
        super::enable_memory_and_bus_master(config, address);
        bars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_placed_aligned_inside_the_window() {
        let mut allocator = WindowAllocator::new([Some((0x1000_0000, 0x1020_0000)), None]);
        let bar = |size: u64| Bar {
            index: 0,
            kind: BarKind::Memory32,
            prefetchable: false,
            address: 0,
            size,
        };
        assert_eq!(allocator.place(&bar(0x1000)), Some(0x1000_0000));
        assert_eq!(allocator.place(&bar(0x10_0000)), Some(0x1010_0000));
        assert_eq!(allocator.place(&bar(0x1000)), None);
        let prefetchable = Bar {
            prefetchable: true,
            ..bar(0x1000)
        };
        assert_eq!(allocator.place(&prefetchable), None);
    }
}
