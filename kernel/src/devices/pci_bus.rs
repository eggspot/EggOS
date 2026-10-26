use super::{DeviceNode, DeviceTable, DmaCoherence, Identity, Probe};
use eggos_drivers::pci::ecam::{Ecam, ecam_length};
use eggos_drivers::pci::{self, BarKind, ConfigSpace, mcfg};
use eggos_hal::{PhysicalRange, Platform};

pub(super) fn discover<P: Platform>(
    probe: &mut super::BootProbe<'_, P>,
    mcfg: &[u8],
) -> Result<(), &'static str> {
    let Some(region) = mcfg::regions(mcfg).find(|region| region.segment == 0) else {
        return Err("the MCFG lists no PCI segment 0");
    };
    let window = PhysicalRange::from_start_length(
        region.base + (u64::from(region.start_bus) << 20),
        ecam_length(region.start_bus, region.end_bus),
    );
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
