//! Intel's Wi-Fi 6 and 6E adapters on PCI (D274; the driver is `eggos_drivers::wifi::iwx`,
//! D272): the probe finds the adapter and keeps its registers; once the file systems are mounted
//! a kernel thread waits for the `wifi-firmware` component's file, powers the adapter up and
//! hands it the firmware. What it reached shows in `/proc/driver/wifi`.

use super::dma::DmaRegion;
use super::{Deadline, DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::linux::exec::ProgramBytes;
use crate::sync::SpinLock;
use crate::vfs::{self, NodeKind};
use alloc::vec::Vec;
use core::time::Duration;
use eggos_drivers::dma::DmaAllocator;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::wifi::iwx::devices::{self, Adapter, VENDOR_INTEL};
use eggos_drivers::wifi::iwx::firmware::Firmware;
use eggos_drivers::wifi::iwx::transport::{self, Transport, Wait};
use eggos_hal::Platform;

const NETWORK_CONTROLLER: u8 = 0x02;
const OTHER_NETWORK: u8 = 0x80;
const SUBSYSTEM_IDS: u16 = 0x2c;
const HW_REV: usize = 0x28;
const LARGEST_FIRMWARE: usize = 4 << 20;
const FIRMWARE_RETRY: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT_MS: u64 = 1000;
const START_TIMEOUT_MS: u64 = 2000;

/// Intel's Wi-Fi adapters (PCI class 02h, subclass 80h, vendor 8086h).
pub static INTEL_WIFI_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "intel-wifi",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: NETWORK_CONTROLLER,
        subclass: OTHER_NETWORK,
    }],
    probe: probe_intel_wifi,
};

struct Found {
    regs: Mmio,
    adapter: Adapter,
}

// SAFETY: the registers are the adapter's alone, mapped for good by the probe, and only the
// bring-up thread uses them once it takes them.
unsafe impl Send for Found {}

static FOUND: SpinLock<Option<Found>> = SpinLock::new(None);
static STATUS: SpinLock<&'static str> = SpinLock::new("no Intel Wi-Fi adapter");
static RUNNING: SpinLock<Option<Running>> = SpinLock::new(None);

struct Running {
    _transport: Transport<Mmio, DmaRegion>,
    firmware: (u8, u8, u8),
}

// SAFETY: as `Found`: the transport's registers and DMA memory are the adapter's alone.
unsafe impl Send for Running {}

/// What the Wi-Fi adapter reached, for `/proc/driver/wifi`.
pub fn status() -> &'static str {
    *STATUS.lock()
}

fn set_status(words: &'static str) {
    *STATUS.lock() = words;
}

fn probe_intel_wifi(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe.table().get(id).ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if function.vendor_id != VENDOR_INTEL {
        return Err(ProbeError::Refused("not an Intel adapter"));
    }
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    if FOUND.lock().is_some() {
        return Err(ProbeError::Refused("an Intel Wi-Fi adapter is already in use"));
    }
    let region = node.regions[0];
    if region.is_empty() {
        return Err(ProbeError::Refused("the adapter's registers are not in memory space"));
    }
    let length = usize::try_from(region.len()).map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let config = probe.pci_config().ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let subdevice = (config.read32(function.address, SUBSYSTEM_IDS) >> 16) as u16;
    pci::enable_memory_and_bus_master(config, function.address);
    let base = probe.map_window(region)?;
    // SAFETY: BAR 0 of an Intel Wi-Fi adapter is its control and status register block, mapped
    // as device memory by `map_window` for as long as the kernel runs; only this driver drives
    // the adapter.
    let regs = unsafe { Mmio::new(base, length) };
    let hw_rev = transport::older_hw_rev(eggos_drivers::io::RegisterIo::read32(&regs, HW_REV));
    let adapter = devices::identify(function.device_id, subdevice, hw_rev)
        .ok_or(ProbeError::Refused("an Intel Wi-Fi adapter this driver does not run"))?;
    set_status("waiting for the file systems");
    *FOUND.lock() = Some(Found { regs, adapter });
    Ok(())
}

struct Regions;

impl DmaAllocator for Regions {
    type Memory = DmaRegion;

    fn allocate(&mut self, bytes: usize) -> Option<DmaRegion> {
        DmaRegion::allocate(bytes).ok()
    }
}

struct Pauses<P: Platform> {
    deadline: Deadline,
    platform: core::marker::PhantomData<P>,
}

impl<P: Platform> Pauses<P> {
    fn within(milliseconds: u64) -> Self {
        Self {
            deadline: Deadline::after::<P>(milliseconds),
            platform: core::marker::PhantomData,
        }
    }
}

impl<P: Platform> Wait for Pauses<P> {
    fn pause(&mut self, microseconds: u32) {
        let mut pause = Deadline::after::<P>(u64::from(microseconds).div_ceil(1000));
        while pause.keep_waiting() {
            core::hint::spin_loop();
        }
    }

    fn keep_waiting(&mut self) -> bool {
        self.deadline.keep_waiting()
    }
}

fn read_firmware(path: &str) -> Option<Vec<u8>> {
    let root = vfs::root();
    let node = vfs::resolve(root, root, path.as_bytes()).ok()?;
    let size = node.attributes().size;
    if node.kind() != NodeKind::Regular || size > LARGEST_FIRMWARE as u64 {
        return None;
    }
    let bytes = ProgramBytes::read(node, size, size).ok()?;
    Some(bytes.bytes().to_vec())
}

/// Starts the thread that brings the adapter the probe found up, once the file systems are
/// mounted; whether there is one.
pub fn start<P: Platform>() -> bool {
    let Some(found) = FOUND.lock().take() else {
        return false;
    };
    set_status("waiting for the wifi-firmware component");
    if crate::thread::spawn::<P, _>(move || bring_up::<P>(found)).is_err() {
        set_status("no thread to start the adapter");
    }
    true
}

fn bring_up<P: Platform>(found: Found) {
    let Found { regs, adapter } = found;
    let path = devices::firmware_path(&adapter);
    let bytes = loop {
        if let Some(bytes) = read_firmware(&path) {
            break bytes;
        }
        crate::thread::sleep::<P>(FIRMWARE_RETRY);
    };
    let firmware = match Firmware::parse(&bytes) {
        Ok(firmware) => firmware,
        Err(_) => return set_status("the firmware file is damaged"),
    };
    let (Ok(rings), Ok(buffers)) = (
        DmaRegion::allocate(transport::RING_BYTES),
        DmaRegion::allocate(transport::RX_BUFFER_AREA_BYTES),
    ) else {
        return set_status("no memory for the adapter's rings");
    };
    let mut transport = Transport::new(regs, adapter, rings, buffers);
    set_status("starting the adapter");
    if let Err(error) = transport.start_hardware(&mut Pauses::<P>::within(START_TIMEOUT_MS)) {
        return set_status(error.description());
    }
    match transport.load_firmware(&firmware, &bytes, &mut Regions, &mut Pauses::<P>::within(LOAD_TIMEOUT_MS)) {
        Ok(_) => {
            set_status(match transport.radio_killed() {
                true => "firmware running; the radio is switched off",
                false => "firmware running",
            });
            *RUNNING.lock() = Some(Running {
                _transport: transport,
                firmware: firmware.version,
            });
        }
        Err(error) => set_status(error.description()),
    }
}

/// The running firmware's major, minor and API version.
pub fn firmware_version() -> Option<(u8, u8, u8)> {
    RUNNING.lock().as_ref().map(|running| running.firmware)
}
