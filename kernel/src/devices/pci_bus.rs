use super::{DeviceNode, DeviceTable, DmaCoherence, Identity, Probe};
use eggos_drivers::pci::ecam::{Ecam, ecam_length};
use eggos_drivers::pci::{self, BarKind, ConfigSpace, mcfg};
use eggos_hal::fdt::Fdt;
use eggos_hal::{PhysicalRange, Platform};

/// The compatible string of a generic ECAM PCI host in a device tree (QEMU's `virt` boards).
const ECAM_HOST: &[u8] = b"pci-host-ecam-generic";
/// A PCI address's space code in `ranges`' first cell: 32-bit and 64-bit memory.
const SPACE_MEMORY32: u32 = 2 << 24;
const SPACE_MEMORY64: u32 = 3 << 24;
const SPACE_MASK: u32 = 3 << 24;
const PREFETCHABLE: u32 = 1 << 30;
const MAX_WINDOWS: usize = 4;

/// A memory window of a PCI host the kernel hands BAR addresses out of (D291).
#[derive(Debug, Clone, Copy, Default)]
struct BarWindow {
    bus: u64,
    cpu: u64,
    size: u64,
    used: u64,
    wide: bool,
    prefetchable: bool,
}

impl BarWindow {
    fn take(&mut self, size: u64) -> Option<(u64, u64)> {
        let start = self.used.checked_next_multiple_of(size)?;
        let end = start.checked_add(size).filter(|&end| end <= self.size)?;
        self.used = end;
        Some((self.bus + start, self.cpu + start))
    }
}

/// Discovers the generic ECAM PCI host a device tree describes (D291): its configuration window
/// from `reg`, its buses from `bus-range`, its memory windows from `ranges`; then enumerates its
/// functions and gives every memory BAR firmware left at 0 an address from those windows,
/// 64-bit BARs from a 64-bit window when there is one. Nothing is found without such a host.
pub(super) fn discover_device_tree<P: Platform>(
    probe: &mut super::BootProbe<'_, P>,
    blob: &'static [u8],
) -> Result<(), &'static str> {
    let tree = Fdt::new(blob).map_err(|_| "the device tree's header is not valid")?;
    let host = tree.nodes().flatten().find(|node| {
        node.enabled && node.compatible.split(|&byte| byte == 0).any(|name| name == ECAM_HOST)
    });
    let Some(host) = host else {
        return Ok(());
    };
    let (window_start, window_size) = host.reg_entries().next().ok_or("the PCI host has no `reg`")?;
    let cell = |bytes: &[u8], at: usize| bytes.get(at * 4..at * 4 + 4).map(|word| u32::from_be_bytes([word[0], word[1], word[2], word[3]]));
    let largest_bus = (window_size >> 20).saturating_sub(1).min(255) as u8;
    let (start_bus, end_bus) = match (cell(host.bus_range, 0), cell(host.bus_range, 1)) {
        (Some(first), Some(last)) => (first.min(255) as u8, (last.min(255) as u8).min(largest_bus)),
        _ => (0, largest_bus),
    };
    let base = window_start
        .checked_add(u64::from(start_bus) << 20)
        .ok_or("the PCI host's ECAM window wraps around")?;
    let window = PhysicalRange::from_start_length(base, ecam_length(start_bus, end_bus));
    let mapped = probe
        .map_window(window)
        .map_err(|_| "the PCI configuration space could not be mapped")?;
    // SAFETY: `map_window` mapped the host's window for the buses it serves as device memory, and
    // windows stay mapped for as long as the kernel runs.
    let ecam = unsafe { Ecam::new(mapped, start_bus, end_bus) };
    let mut windows = [BarWindow::default(); MAX_WINDOWS];
    let mut count = 0;
    let parent_cells = host.address_cells as usize;
    let entry_cells = 3 + parent_cells + 2;
    for entry in host.own_ranges.chunks_exact(entry_cells * 4) {
        let words: [u32; 9] = core::array::from_fn(|index| cell(entry, index).unwrap_or(0));
        let space = words[0] & SPACE_MASK;
        if space != SPACE_MEMORY32 && space != SPACE_MEMORY64 || count == MAX_WINDOWS {
            continue;
        }
        let bus = u64::from(words[1]) << 32 | u64::from(words[2]);
        let cpu = match parent_cells {
            1 => u64::from(words[3]),
            _ => u64::from(words[3]) << 32 | u64::from(words[4]),
        };
        let size_at = 3 + parent_cells;
        let size = u64::from(words[size_at]) << 32 | u64::from(words[size_at + 1]);
        windows[count] = BarWindow {
            bus,
            cpu,
            size,
            used: 0,
            wide: space == SPACE_MEMORY64,
            prefetchable: words[0] & PREFETCHABLE != 0,
        };
        count += 1;
    }
    let windows = &mut windows[..count];
    pci::enumerate(&ecam, start_bus, end_bus, &mut |function| {
        let mut device = DeviceNode::new(Identity::Pci(function));
        device.dma = DmaCoherence::Coherent;
        for bar in pci::bars(&ecam, function.address, function.header_type)
            .into_iter()
            .flatten()
        {
            if bar.kind == BarKind::Io {
                continue;
            }
            let address = match bar.address {
                0 => {
                    let wide = bar.kind == BarKind::Memory64;
                    let chosen = windows
                        .iter_mut()
                        .filter(|window| window.wide == wide || !window.wide)
                        .find_map(|window| window.take(bar.size));
                    let Some((bus, cpu)) = chosen else {
                        continue;
                    };
                    pci::assign(&ecam, function.address, &bar, bus);
                    cpu
                }
                assigned => windows
                    .iter()
                    .find(|window| assigned >= window.bus && assigned - window.bus < window.size)
                    .map_or(assigned, |window| window.cpu + (assigned - window.bus)),
            };
            device.regions[usize::from(bar.index)] = PhysicalRange::from_start_length(address, bar.size);
        }
        probe.table.add(device);
    });
    probe.ecam = Some(ecam);
    Ok(())
}

pub(super) fn discover<P: Platform>(
    probe: &mut super::BootProbe<'_, P>,
    mcfg: &[u8],
) -> Result<(), &'static str> {
    let Some(region) = mcfg::regions(mcfg).find(|region| region.segment == 0) else {
        return Err("the MCFG lists no PCI segment 0");
    };
    let start = region
        .base
        .checked_add(u64::from(region.start_bus) << 20)
        .ok_or("the MCFG's ECAM window wraps around")?;
    let window =
        PhysicalRange::from_start_length(start, ecam_length(region.start_bus, region.end_bus));
    let base = probe
        .map_window(window)
        .map_err(|_| "the PCI configuration space could not be mapped")?;
    // SAFETY: `map_window` mapped the MCFG's window for the buses it names as device memory, and
    // windows stay mapped for as long as the kernel runs.
    let ecam = unsafe { Ecam::new(base, region.start_bus, region.end_bus) };
    add_functions(&ecam, region.start_bus, region.end_bus, probe.table);
    probe.ecam = Some(ecam);
    Ok(())
}

fn add_functions<C: ConfigSpace + ?Sized>(
    config: &C,
    start_bus: u8,
    end_bus: u8,
    table: &mut DeviceTable,
) {
    pci::enumerate(config, start_bus, end_bus, &mut |function| {
        let mut device = DeviceNode::new(Identity::Pci(function));
        device.dma = DmaCoherence::Coherent;
        for bar in pci::bars(config, function.address, function.header_type)
            .into_iter()
            .flatten()
        {
            if bar.kind != BarKind::Io {
                device.regions[usize::from(bar.index)] =
                    PhysicalRange::from_start_length(bar.address, bar.size);
            }
        }
        table.add(device);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_drivers::pci::Address;

    #[derive(Default)]
    struct Q35 {
        functions: std::collections::BTreeMap<(Address, u16), u32>,
        sizing: std::cell::RefCell<std::collections::BTreeSet<(Address, u16)>>,
    }

    impl Q35 {
        fn with(mut self, address: Address, id: u32, class: u32, bars: &[(u16, u32, u32)]) -> Self {
            self.functions.insert((address, 0), id);
            self.functions.insert((address, 8), class);
            for &(offset, value, mask) in bars {
                self.functions.insert((address, offset), value);
                self.functions.insert((address, offset | 0x8000), mask);
            }
            self
        }
    }

    impl ConfigSpace for Q35 {
        fn read32(&self, address: Address, offset: u16) -> u32 {
            if !self.functions.contains_key(&(address, 0)) {
                return u32::MAX;
            }
            if self.sizing.borrow().contains(&(address, offset)) {
                return self
                    .functions
                    .get(&(address, offset | 0x8000))
                    .copied()
                    .unwrap_or(0);
            }
            self.functions.get(&(address, offset)).copied().unwrap_or(0)
        }
        fn write32(&self, address: Address, offset: u16, value: u32) {
            if (0x10..0x28).contains(&offset) {
                let mut sizing = self.sizing.borrow_mut();
                if value == u32::MAX {
                    sizing.insert((address, offset));
                } else {
                    sizing.remove(&(address, offset));
                }
            }
        }
    }

    #[test]
    fn functions_become_devices_with_their_memory_bars_by_number() {
        let host = Address::new(0, 0, 0).unwrap();
        let rng = Address::new(0, 3, 0).unwrap();
        let q35 = Q35::default()
            .with(host, 0x29c0_8086, 0x0600_0000, &[])
            .with(
                rng,
                0x1044_1af4,
                0x00ff_0001,
                &[
                    (0x10, 0xc041, 0xffff_ffe1),
                    (0x14, 0xfebd_1000, 0xffff_f000),
                    (0x20, 0xfe00_000c, 0xffff_c00c),
                    (0x24, 0, 0xffff_ffff),
                ],
            );
        let mut table = DeviceTable::new();
        add_functions(&q35, 0, 255, &mut table);
        let devices: Vec<_> = table.iter().map(|(_, node)| *node).collect();
        assert_eq!(devices.len(), 2);
        let Identity::Pci(bridge) = devices[0].identity else {
            panic!("not a PCI device");
        };
        assert_eq!((bridge.class, bridge.subclass), (6, 0));
        assert_eq!(devices[0].regions().count(), 0);
        let regions = devices[1].regions;
        assert!(regions[0].is_empty());
        assert_eq!(
            regions[1],
            PhysicalRange::from_start_length(0xfebd_1000, 0x1000)
        );
        assert_eq!(
            regions[4],
            PhysicalRange::from_start_length(0xfe00_0000, 0x4000)
        );
        assert_eq!(devices[1].dma, DmaCoherence::Coherent);
    }
}
