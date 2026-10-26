use super::{DeviceNode, DeviceTable, DmaCoherence, Identity};
use eggos_hal::PhysicalRange;
use eggos_hal::fdt::{Fdt, Node};

const MAX_CONTROLLERS: usize = 32;

pub(super) fn discover(
    blob: &'static [u8],
    table: &mut DeviceTable,
    coherent_by_default: bool,
) -> Result<(), &'static str> {
    let tree = Fdt::new(blob).map_err(|_| "the device tree's header is not valid")?;
    let mut controllers = [(0u32, 0u32); MAX_CONTROLLERS];
    let mut known = 0;
    for node in tree.nodes() {
        let node = node.map_err(|_| "the device tree is malformed")?;
        if let (Some(phandle), Some(cells)) = (node.phandle, node.interrupt_cells)
            && known < MAX_CONTROLLERS
        {
            controllers[known] = (phandle, cells);
            known += 1;
        }
    }
    let controllers = &controllers[..known];
    let cells_of = |phandle| {
        controllers
            .iter()
            .find_map(|&(known, cells)| (known == phandle).then_some(cells))
    };
    let mut cpus_depth = None;
    for node in tree.nodes() {
        let node = node.map_err(|_| "the device tree is malformed")?;
        match cpus_depth {
            Some(depth) if node.depth > depth => continue,
            _ => cpus_depth = None,
        }
        if node.depth == 1 && node.name == b"cpus" {
            cpus_depth = Some(node.depth);
            continue;
        }
        if node.depth == 0 || !node.enabled || node.compatible.is_empty() {
            continue;
        }
        table.add(device(&node, &cells_of, coherent_by_default));
    }
    Ok(())
}

fn device(
    node: &Node<'static>,
    cells_of: &impl Fn(u32) -> Option<u32>,
    coherent_by_default: bool,
) -> DeviceNode {
    let mut device = DeviceNode::new(Identity::DeviceTree {
        name: node.name,
        compatible: node.compatible,
    });
    if node.cpu_addressable {
        for (slot, (address, size)) in device.regions.iter_mut().zip(node.reg_entries()) {
            *slot = PhysicalRange::from_start_length(address, size);
        }
    }
    for (slot, interrupt) in device
        .interrupts
        .iter_mut()
        .zip(node.interrupt_specifiers(cells_of))
    {
        *slot = Some(interrupt);
    }
    let coherent = node.dma_coherent || (coherent_by_default && !node.dma_noncoherent);
    device.dma = if coherent {
        DmaCoherence::Coherent
    } else {
        DmaCoherence::NonCoherent
    };
    device
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Binding;

    const QEMU_AARCH64_VIRT: &[u8] =
        include_bytes!("../../../hal/src/fdt/testdata/qemu-aarch64-virt.dtb");
    const QEMU_RISCV64_VIRT: &[u8] =
        include_bytes!("../../../hal/src/fdt/testdata/qemu-riscv64-virt.dtb");

    fn discovered(blob: &'static [u8], coherent_by_default: bool) -> DeviceTable {
        let mut table = DeviceTable::new();
        discover(blob, &mut table, coherent_by_default).unwrap();
        table
    }

    fn compatible_with<'a>(table: &'a DeviceTable, wanted: &[u8]) -> Vec<&'a DeviceNode> {
        table
            .iter()
            .map(|(_, node)| node)
            .filter(|node| match node.identity {
                Identity::DeviceTree { compatible, .. } => compatible
                    .split(|&byte| byte == 0)
                    .any(|string| string == wanted),
                _ => false,
            })
            .collect()
    }

    #[test]
    fn qemu_aarch64_virt_has_32_coherent_virtio_mmio_slots_on_gic_spis() {
        let table = discovered(QEMU_AARCH64_VIRT, false);
        assert_eq!(table.dropped(), 0);
        let slots = compatible_with(&table, b"virtio,mmio");
        assert_eq!(slots.len(), 32);
        for node in &slots {
            assert_eq!(node.dma, DmaCoherence::Coherent);
            assert_eq!(node.binding, Binding::Unbound);
            let region = node.regions().next().unwrap();
            assert_eq!(region.len(), 0x200);
            assert!((0xa00_0000..0xa00_4000).contains(&region.start));
            let interrupt = node.interrupts[0].unwrap();
            assert_eq!(interrupt.cells()[0], 0);
            assert_eq!(
                interrupt.cells()[1],
                0x10 + (region.start - 0xa00_0000) as u32 / 0x200
            );
        }
        assert_eq!(compatible_with(&table, b"arm,pl011").len(), 1);
        assert_eq!(compatible_with(&table, b"arm,cortex-a15-gic").len(), 1);
        assert!(compatible_with(&table, b"arm,cortex-a72").is_empty());
    }

    #[test]
    fn qemu_riscv64_virt_has_8_virtio_mmio_slots_on_plic_sources() {
        let table = discovered(QEMU_RISCV64_VIRT, true);
        let slots = compatible_with(&table, b"virtio,mmio");
        assert_eq!(slots.len(), 8);
        for node in &slots {
            assert_eq!(node.dma, DmaCoherence::Coherent);
            let region = node.regions().next().unwrap();
            let source = (region.start - 0x1000_0000) / 0x1000;
            assert_eq!(node.interrupts[0].unwrap().cells(), [source as u32]);
        }
        assert_eq!(compatible_with(&table, b"riscv,plic0").len(), 1);
        assert!(compatible_with(&table, b"riscv,cpu-intc").is_empty());
        let without_default = discovered(QEMU_RISCV64_VIRT, false);
        assert!(
            compatible_with(&without_default, b"virtio,mmio")
                .iter()
                .all(|node| node.dma == DmaCoherence::NonCoherent)
        );
    }

    #[test]
    fn a_full_table_counts_the_devices_it_drops() {
        let mut table = DeviceTable::new();
        for _ in 0..super::super::MAX_DEVICES - 3 {
            table.add(DeviceNode::new(Identity::Virtio {
                device_type: 4,
                transport: 0,
            }));
        }
        discover(QEMU_AARCH64_VIRT, &mut table, false).unwrap();
        assert_eq!(table.len(), super::super::MAX_DEVICES);
        assert!(table.dropped() > 30);
    }

    #[test]
    fn a_malformed_tree_is_reported() {
        let mut table = DeviceTable::new();
        assert!(discover(&QEMU_RISCV64_VIRT[..100], &mut table, true).is_err());
        let mut corrupt = QEMU_RISCV64_VIRT.to_vec();
        let structure = u32::from_be_bytes(corrupt[8..12].try_into().unwrap()) as usize;
        corrupt[structure + 3] = 0x7f;
        let corrupt: &'static [u8] = corrupt.leak();
        assert!(discover(corrupt, &mut table, true).is_err());
    }
}
