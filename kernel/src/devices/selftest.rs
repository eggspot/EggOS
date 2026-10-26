use super::dma::DmaRegion;
use super::{Binding, DeviceNode, Identity};
use crate::memory::{FRAMES, Memory};
use eggos_drivers::dma::DmaMemory;
use eggos_hal::Platform;

fn devices() -> alloc::vec::Vec<DeviceNode> {
    let mut found = alloc::vec::Vec::new();
    super::for_each(&mut |_, node| found.push(*node));
    found
}

fn compatible(node: &DeviceNode, wanted: &[u8]) -> bool {
    match node.identity {
        Identity::DeviceTree { compatible, .. } => compatible
            .split(|&byte| byte == 0)
            .any(|string| string == wanted),
        _ => false,
    }
}

fn entropy_devices(found: &[DeviceNode]) -> usize {
    found
        .iter()
        .filter(|node| {
            matches!(
                node.identity,
                Identity::Virtio {
                    device_type: eggos_drivers::virtio::device_type::ENTROPY,
                    ..
                }
            )
        })
        .count()
}

fn virtio_mmio_slots(found: &[DeviceNode], expected: usize) -> Result<(), &'static str> {
    let slots: alloc::vec::Vec<_> = found
        .iter()
        .filter(|node| compatible(node, b"virtio,mmio"))
        .collect();
    if slots.len() != expected {
        return Err("the device tree did not give the machine's virtio-mmio slots");
    }
    if slots
        .iter()
        .any(|node| node.regions().next().is_none() || node.interrupts[0].is_none())
    {
        return Err("a virtio-mmio slot has no registers or no interrupt");
    }
    if cfg!(feature = "virtio")
        && slots
            .iter()
            .any(|node| node.binding != Binding::Bound("virtio-mmio"))
    {
        return Err("a virtio-mmio slot was not bound (a legacy device?)");
    }
    Ok(())
}

fn q35_functions(found: &[DeviceNode]) -> Result<(), &'static str> {
    let functions = || {
        found.iter().filter_map(|node| match node.identity {
            Identity::Pci(function) => Some((function, node)),
            _ => None,
        })
    };
    if !functions().any(|(f, _)| (f.class, f.subclass) == (6, 0) && f.address.device == 0) {
        return Err("PCI enumeration found no host bridge at 00:00.0");
    }
    let Some((_, rng)) = functions().find(|(f, _)| (f.vendor_id, f.device_id) == (0x1af4, 0x1044))
    else {
        return Err("PCI enumeration found no modern virtio-rng-pci function");
    };
    if rng.regions().next().is_none() || rng.regions().any(|region| region.start == 0) {
        return Err("the virtio-rng-pci function's BARs were not assigned");
    }
    if cfg!(feature = "virtio") && rng.binding != Binding::Bound("virtio-pci") {
        return Err("the virtio-pci driver did not bind the virtio-rng-pci function");
    }
    Ok(())
}

pub(crate) fn devices_are_discovered() -> Result<(), &'static str> {
    let found = devices();
    let machine = if cfg!(all(target_os = "none", target_arch = "aarch64")) {
        virtio_mmio_slots(&found, 32).and_then(|()| {
            let gic = found
                .iter()
                .any(|node| compatible(node, b"arm,cortex-a15-gic"));
            let uart = found.iter().any(|node| compatible(node, b"arm,pl011"));
            (gic && uart)
                .then_some(())
                .ok_or("the device tree did not give the GIC and the PL011")
        })
    } else if cfg!(all(target_os = "none", target_arch = "riscv64")) {
        virtio_mmio_slots(&found, 8).and_then(|()| {
            found
                .iter()
                .any(|node| compatible(node, b"riscv,plic0"))
                .then_some(())
                .ok_or("the device tree did not give the PLIC")
        })
    } else if cfg!(all(target_os = "none", target_arch = "x86_64")) {
        q35_functions(&found)
    } else {
        return Ok(());
    };
    machine?;
    if cfg!(feature = "virtio") && entropy_devices(&found) != 1 {
        return Err("no virtio entropy device behind the transports");
    }
    Ok(())
}

#[cfg(any(test, feature = "virtio-rng"))]
pub(crate) fn virtio_rng_returns_bytes<P: Platform>() -> Result<(), &'static str> {
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];
    let mut deadline = super::Deadline::after::<P>(1000);
    let read = super::virtio::read_random(&mut first, &mut deadline);
    if read.is_none() && !cfg!(target_os = "none") {
        return Ok(());
    }
    if read != Some(32) || super::virtio::read_random(&mut second, &mut deadline) != Some(32) {
        return Err("the virtio entropy device did not return 32 bytes");
    }
    if first == [0; 32] || first == second {
        return Err("the virtio entropy device returned zeros or the same bytes twice");
    }
    Ok(())
}

fn free_frames() -> Option<u64> {
    FRAMES.lock().as_ref().map(|frames| frames.free_frames())
}

pub(crate) fn dma_regions_are_zeroed_and_freed_after_reset<P: Platform>(
    _memory: &mut Memory<P>,
) -> Result<(), &'static str> {
    let before = free_frames().ok_or("there is no frame allocator")?;
    let region = DmaRegion::allocate(3 * 4096).map_err(|_| "no memory for a DMA region")?;
    let size_and_alignment = region.len() == 4 * 4096
        && region.dma_addr().get().is_multiple_of(4 * 4096)
        && (region.as_mut_ptr() as usize).is_multiple_of(4096);
    let zeroed = (0..region.len()).all(|offset| {
        // SAFETY: inside the region, which nothing but this test uses.
        unsafe { region.as_mut_ptr().wrapping_add(offset).read_volatile() == 0 }
    });
    // SAFETY: the region was never given to a device.
    unsafe { region.free() };
    if !size_and_alignment || !zeroed {
        return Err("a DMA region was not whole, aligned frames, zeroed");
    }
    if free_frames() != Some(before) {
        return Err("a DMA region's frames were not all given back");
    }
    #[cfg(any(test, feature = "virtio-rng"))]
    reset_and_reinitialize_the_entropy_device::<P>(before)?;
    Ok(())
}

#[cfg(any(test, feature = "virtio-rng"))]
fn reset_and_reinitialize_the_entropy_device<P: Platform>(before: u64) -> Result<(), &'static str> {
    use eggos_drivers::virtio::rng::{MEMORY_BYTES, VirtioRng};
    let Some(rng) = super::virtio::take_rng() else {
        return Ok(());
    };
    let mut deadline = super::Deadline::after::<P>(1000);
    let (transport, memory, reset) = rng
        .reset(&mut || deadline.keep_waiting())
        .map_err(|_| "the virtio entropy device's status did not read back 0 after a reset")?;
    let frames = (memory.len() / 4096) as u64;
    memory.free_after_reset(&reset);
    if free_frames() != Some(before + frames) {
        return Err("the reset device's DMA region was not given back");
    }
    let memory = DmaRegion::allocate(MEMORY_BYTES).map_err(|_| "no memory for a DMA region")?;
    let mut rng = VirtioRng::new(transport, memory, &mut || deadline.keep_waiting())
        .map_err(|_| "the virtio entropy device could not be set up again after its reset")?;
    let mut bytes = [0u8; 32];
    let read = rng.read(&mut bytes, &mut || deadline.keep_waiting());
    super::virtio::put_rng(rng);
    if read != Ok(32) {
        return Err("the virtio entropy device set up again returned no bytes");
    }
    if free_frames() != Some(before) {
        return Err("setting the virtio entropy device up again took more than its region");
    }
    Ok(())
}
