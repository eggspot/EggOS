//! Disk controllers on PCI (D238): NVMe and AHCI (SATA), each disk installed as one of the
//! kernel's disks ([`crate::block`]), polled.

use super::dma::DmaRegion;
use super::{DeviceId, DeviceNode, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::block::{BlockDevice, BlockError};
use crate::devices::Deadline;
use alloc::boxed::Box;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::storage::{self, ahci, nvme};
use eggos_hal::PhysicalRange;

const START_TIMEOUT_MS: u64 = 10_000;
const MASS_STORAGE: u8 = 0x01;
const SATA: u8 = 0x06;
const NON_VOLATILE_MEMORY: u8 = 0x08;
const AHCI_INTERFACE: u8 = 0x01;
const NVME_INTERFACE: u8 = 0x02;
const AHCI_BAR: usize = 5;
const NVME_BAR: usize = 0;

/// NVM Express controllers (PCI class 01h, subclass 08h, interface 02h).
pub static NVME_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "nvme",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: MASS_STORAGE,
        subclass: NON_VOLATILE_MEMORY,
    }],
    probe: probe_nvme,
};

/// AHCI SATA controllers (PCI class 01h, subclass 06h, interface 01h).
pub static AHCI_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "ahci",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: MASS_STORAGE,
        subclass: SATA,
    }],
    probe: probe_ahci,
};

fn block_error(error: storage::Error) -> BlockError {
    match error {
        storage::Error::OutOfRange => BlockError::OutOfRange,
        storage::Error::ReadOnly => BlockError::ReadOnly,
        error => BlockError::Device(error.description()),
    }
}

struct NvmeDisk(nvme::Nvme<Mmio, DmaRegion>);

impl BlockDevice for NvmeDisk {
    fn sectors(&self) -> u64 {
        self.0.sectors()
    }
    fn is_read_only(&self) -> bool {
        false
    }
    fn logical_block_bytes(&self) -> usize {
        self.0.block_bytes()
    }
    fn read(&mut self, sector: u64, buffer: &mut [u8], deadline: &mut Deadline) -> Result<(), BlockError> {
        self.0
            .read(sector, buffer, &mut || deadline.keep_waiting())
            .map_err(block_error)
    }
    fn write(&mut self, sector: u64, buffer: &[u8], deadline: &mut Deadline) -> Result<(), BlockError> {
        self.0
            .write(sector, buffer, &mut || deadline.keep_waiting())
            .map_err(block_error)
    }
    fn flush(&mut self, deadline: &mut Deadline) -> Result<(), BlockError> {
        self.0
            .flush(&mut || deadline.keep_waiting())
            .map_err(block_error)
    }
}

struct SataDisk(ahci::AhciDisk<Mmio, DmaRegion>);

impl BlockDevice for SataDisk {
    fn sectors(&self) -> u64 {
        self.0.sectors()
    }
    fn is_read_only(&self) -> bool {
        false
    }
    fn logical_block_bytes(&self) -> usize {
        self.0.block_bytes()
    }
    fn read(&mut self, sector: u64, buffer: &mut [u8], deadline: &mut Deadline) -> Result<(), BlockError> {
        self.0
            .read(sector, buffer, &mut || deadline.keep_waiting())
            .map_err(block_error)
    }
    fn write(&mut self, sector: u64, buffer: &[u8], deadline: &mut Deadline) -> Result<(), BlockError> {
        self.0
            .write(sector, buffer, &mut || deadline.keep_waiting())
            .map_err(block_error)
    }
    fn flush(&mut self, deadline: &mut Deadline) -> Result<(), BlockError> {
        self.0
            .flush(&mut || deadline.keep_waiting())
            .map_err(block_error)
    }
}

fn pci_function(probe: &mut dyn Probe, id: DeviceId) -> Result<(DeviceNode, pci::Function), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused(
            "the device's DMA is not coherent, which the kernel does not support yet",
        ));
    }
    Ok((node, function))
}

fn map_bar(probe: &mut dyn Probe, node: &DeviceNode, bar: usize) -> Result<(usize, usize), ProbeError> {
    let region: PhysicalRange = node.regions[bar];
    if region.is_empty() {
        return Err(ProbeError::Refused("the controller's registers are not in memory space"));
    }
    let length = usize::try_from(region.len()).map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let base = probe.map_window(region)?;
    Ok((base, length))
}

fn start_device(probe: &mut dyn Probe, function: &pci::Function) -> Result<(), ProbeError> {
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    pci::enable_memory_and_bus_master(config, function.address);
    Ok(())
}

fn give_back(memory: Option<DmaRegion>) {
    if let Some(memory) = memory {
        // SAFETY: the drivers hand the memory back only when the controller never had it or was
        // disabled after.
        unsafe { memory.free() };
    }
}

fn probe_nvme(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let (node, function) = pci_function(probe, id)?;
    if function.prog_if != NVME_INTERFACE {
        return Err(ProbeError::Refused("a non-volatile memory controller that is not NVMe"));
    }
    if !crate::block::has_room() {
        return Err(ProbeError::Refused("no room for another disk"));
    }
    let (base, length) = map_bar(probe, &node, NVME_BAR)?;
    start_device(probe, &function)?;
    // SAFETY: BAR 0 of an NVMe controller is its register block, mapped as device memory by
    // `map_window` for as long as the kernel runs, and only this driver drives the controller.
    let regs = unsafe { Mmio::new(base, length) };
    let memory = DmaRegion::allocate(nvme::MEMORY_BYTES)
        .map_err(|_| ProbeError::Refused("no memory for the controller's queues"))?;
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let controller = nvme::Nvme::new(regs, memory, &mut || deadline.keep_waiting()).map_err(|(error, memory)| {
        give_back(memory);
        ProbeError::Refused(error.description())
    })?;
    crate::block::install(Box::new(NvmeDisk(controller)))
        .map_err(|_| ProbeError::Refused("no room for another disk"))
}

fn probe_ahci(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let (node, function) = pci_function(probe, id)?;
    if function.prog_if != AHCI_INTERFACE {
        return Err(ProbeError::Refused("a SATA controller that is not in AHCI mode"));
    }
    let (base, length) = map_bar(probe, &node, AHCI_BAR)?;
    start_device(probe, &function)?;
    // SAFETY: BAR 5 of an AHCI controller is its register block (ABAR), mapped as device memory
    // by `map_window` for as long as the kernel runs; only this driver drives it.
    let controller_regs = unsafe { Mmio::new(base, length) };
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let controller = ahci::take_controller(&controller_regs, &mut || deadline.keep_waiting());
    let mut installed = 0;
    let mut last_refusal = "no SATA disk is connected";
    for port in 0..ahci::MAX_PORTS {
        if controller.disk_ports & (1 << port) == 0 {
            continue;
        }
        if !crate::block::has_room() {
            last_refusal = "no room for another disk";
            break;
        }
        let offset = ahci::PORT_REGISTERS + port * ahci::PORT_REGISTER_BYTES;
        if offset + ahci::PORT_REGISTER_BYTES > length {
            break;
        }
        // SAFETY: the port's registers inside the mapped ABAR; each port's block is driven only
        // by its own disk, and the controller-wide registers are not touched again.
        let regs = unsafe { Mmio::new(base + offset, ahci::PORT_REGISTER_BYTES) };
        let Ok(memory) = DmaRegion::allocate(ahci::MEMORY_BYTES) else {
            last_refusal = "no memory for a SATA port";
            continue;
        };
        let mut deadline = probe.deadline(START_TIMEOUT_MS);
        match ahci::AhciDisk::new(regs, memory, controller.addresses_64_bit, &mut || deadline.keep_waiting()) {
            Ok(disk) => {
                if crate::block::install(Box::new(SataDisk(disk))).is_ok() {
                    installed += 1;
                }
            }
            Err((error, memory)) => {
                give_back(memory);
                last_refusal = error.description();
            }
        }
    }
    if installed == 0 {
        return Err(ProbeError::Refused(last_refusal));
    }
    Ok(())
}
