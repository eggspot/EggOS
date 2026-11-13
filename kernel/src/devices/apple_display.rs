//! Apple silicon's display through its DCP (D547): the coprocessor started and woken, RTKit's
//! handshake answered with shared memory the DCP's DART maps, the iBoot display service found
//! on EPIC, the screen powered and set to the mode iBoot would choose, and a framebuffer of the
//! kernel's own mapped at one address in the DCP's DART and the display pipe's, shown, and
//! installed as the display. The device tree gives the DCP (`apple,dcp`: its coprocessor
//! registers, mailbox and DART stream) and the display pipe (`apple,display-subsystem`: its
//! DART stream). Until it is lit the screen keeps the firmware's framebuffer (D542).

use super::dma::DmaRegion;
use super::{DeviceId, DriverDescriptor, Match, Probe, ProbeError, ProbePass};
use crate::display::{Mode, Rect, Scanout};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use eggos_drivers::apple::dart::{self, Dart, DartMemory, PAGE_BYTES};
use eggos_drivers::apple::dcp::{self, DisplayService, Firmware, Surface, surface_format};
use eggos_drivers::apple::epic::{EpicError, Link, SharedBuffer};
use eggos_drivers::apple::mailbox::{self, Mailbox};
use eggos_drivers::apple::rtkit::{Rtkit, RtkitError, SharedMemory};
use eggos_drivers::dma::DmaMemory;
use eggos_drivers::io::{Mmio, RegisterIo};
use eggos_hal::PhysicalRange;
use eggos_hal::fdt::{self, Node};

pub static APPLE_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "apple-display",
    pass: ProbePass::Device,
    matches: &[Match::Compatible(b"apple,dcp")],
    probe,
};

const DISPLAY_SUBSYSTEM: &[u8] = b"apple,display-subsystem";
const CPU_CONTROL: usize = 0x44;
const CPU_START: u32 = 0x10;
const COPROCESSOR_BYTES: u64 = 0x1000;
const MAILBOX_BYTES: u64 = 0x4000;
const DART_BYTES: u64 = 0x4000;
const BOOT_TIMEOUT_MS: u64 = 2_000;
const FIRST_SHARED_ADDRESS: u64 = 0x1_0000;
const BYTES_PER_PIXEL: u32 = 4;
const FIRMWARE_VERSION: &[u8] = b"asahi,os-fw-version";

struct KernelTables {
    regions: Vec<DmaRegion>,
}

// SAFETY: each new table is a fresh, zeroed `DmaRegion` of `PAGE_BYTES` (a power-of-two block
// of frames, aligned to its size), kept in `regions` for as long as the DART lives and given
// to nothing else; `table_at` reaches a firmware table through the direct map, which covers
// all RAM, and the DART driver is its only user once the kernel took the DART.
unsafe impl DartMemory for KernelTables {
    fn new_table(&mut self) -> Option<(usize, u64)> {
        let region = DmaRegion::allocate(PAGE_BYTES as usize).ok()?;
        let table = (region.as_mut_ptr() as usize, region.physical());
        self.regions.push(region);
        Some(table)
    }

    fn table_at(&mut self, physical: u64) -> Option<usize> {
        let offset = crate::memory::FRAMES
            .lock()
            .as_ref()?
            .physical_memory_offset();
        Some((physical as usize).wrapping_add(offset))
    }
}

struct DcpMemory {
    dart: Dart<Mmio, KernelTables>,
    regions: Vec<DmaRegion>,
}

impl DcpMemory {
    fn share(&mut self, bytes: u64, wanted: u64) -> Option<SharedBuffer> {
        let bytes = bytes.max(1).next_multiple_of(PAGE_BYTES);
        let region = DmaRegion::allocate(bytes as usize).ok()?;
        let address = match wanted {
            0 => self.dart.find_free(FIRST_SHARED_ADDRESS, bytes)?,
            given => given,
        };
        self.dart.map(address, region.physical(), bytes).ok()?;
        let buffer = SharedBuffer {
            base: region.as_mut_ptr() as usize,
            bytes: bytes as usize,
            device_address: address,
        };
        self.regions.push(region);
        Some(buffer)
    }
}

impl SharedMemory for DcpMemory {
    fn shared_memory(&mut self, _endpoint: u8, bytes: u64, address: u64) -> Option<u64> {
        self.share(bytes, address)
            .map(|buffer| buffer.device_address)
    }
}

struct DcpLink<'a> {
    rtkit: &'a mut Rtkit<Mmio>,
    memory: &'a mut DcpMemory,
    waiting: VecDeque<(u8, u64)>,
    deadline: super::Deadline,
}

impl DcpLink<'_> {
    fn pump(&mut self) -> Result<(), RtkitError> {
        let Self {
            rtkit,
            memory,
            waiting,
            ..
        } = self;
        rtkit.poll(*memory, &mut |endpoint, payload| {
            waiting.push_back((endpoint, payload))
        })
    }

    fn until_running(&mut self) -> Result<(), RtkitError> {
        while !self.rtkit.is_running() {
            if !self.deadline.keep_waiting() {
                return Err(RtkitError::NoSuchEndpoint);
            }
            self.pump()?;
        }
        Ok(())
    }
}

// SAFETY: every buffer comes from `DcpMemory::share`: a fresh `DmaRegion` (direct-mapped,
// coherent, owned by `memory` for as long as the DCP runs) mapped at its device address in the
// DCP's DART, used only by the DCP and this driver.
unsafe impl Link for DcpLink<'_> {
    fn send(&mut self, endpoint: u8, payload: u64) -> Result<(), RtkitError> {
        self.rtkit.send(endpoint, payload)
    }

    fn next(&mut self, endpoint: u8) -> Result<Option<u64>, RtkitError> {
        loop {
            if let Some(index) = self.waiting.iter().position(|&(from, _)| from == endpoint) {
                return Ok(self.waiting.remove(index).map(|(_, payload)| payload));
            }
            if !self.deadline.keep_waiting() {
                return Ok(None);
            }
            self.pump()?;
        }
    }

    fn allocate(&mut self, bytes: usize) -> Option<SharedBuffer> {
        self.memory.share(bytes as u64, 0)
    }
}

struct AppleScanout {
    pixels: *mut u32,
    mode: Mode,
}

// SAFETY: the pixels are a frame block the kernel owns for good, mapped write-combining for
// this scanout alone; the display's lock serializes every use.
unsafe impl Send for AppleScanout {}

impl Scanout for AppleScanout {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        let count = self.mode.width as usize * self.mode.height as usize;
        // SAFETY: `pixels` maps `count` pixels of the framebuffer for as long as the kernel
        // runs, owned by this scanout, and `&mut self` makes the borrow unique.
        unsafe { core::slice::from_raw_parts_mut(self.pixels, count) }
    }

    fn flush(
        &mut self,
        rects: &[Rect],
        _deadline: &mut super::Deadline,
    ) -> Result<usize, &'static str> {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Ok(rects.len())
    }
}

fn reference(cells: &[u8]) -> Option<(u32, Option<u32>)> {
    let word = |at: usize| Some(u32::from_be_bytes(cells.get(at..at + 4)?.try_into().ok()?));
    Some((word(0)?, word(4)))
}

fn map_registers(probe: &mut dyn Probe, node: &Node<'_>, bytes: u64) -> Result<Mmio, ProbeError> {
    let (start, length) = node
        .reg_entries()
        .next()
        .ok_or(ProbeError::Refused("a node has no registers"))?;
    let length = match length {
        0 => bytes,
        given => given,
    };
    let virt = probe.map_window(PhysicalRange::from_start_length(start, length))?;
    // SAFETY: `map_window` mapped the node's register block as device memory for good; only
    // this driver reaches it.
    Ok(unsafe { Mmio::new(virt, length as usize) })
}

fn dart_of(
    probe: &mut dyn Probe,
    tree: &'static [u8],
    node: &Node<'_>,
) -> Result<Dart<Mmio, KernelTables>, ProbeError> {
    let (phandle, stream) =
        reference(node.iommus).ok_or(ProbeError::Refused("the node names no DART"))?;
    let iommu = fdt::node_with_phandle(tree, phandle)
        .ok_or(ProbeError::Refused("the DART is not in the device tree"))?;
    let kind = dart::Kind::of_compatible(iommu.compatible_strings())
        .ok_or(ProbeError::Refused("a DART of a kind EggOS does not know"))?;
    let registers = map_registers(probe, &iommu, DART_BYTES)?;
    let tables = KernelTables {
        regions: Vec::new(),
    };
    let stream = stream.unwrap_or(0) as u8;
    Dart::new(registers, kind, stream, tables, true)
        .map_err(|_| ProbeError::Refused("the DART could not be taken"))
}

fn firmware_of(tree: &'static [u8]) -> Firmware {
    let version = fdt::Fdt::new(tree)
        .ok()
        .and_then(|parsed| parsed.property_at(b"/chosen", FIRMWARE_VERSION));
    let major_minor = version.and_then(|text| {
        let text = text.split(|&byte| byte == 0).next()?;
        let mut parts = text.split(|&byte| byte == b'.');
        let number = |part: &[u8]| core::str::from_utf8(part).ok()?.parse::<u32>().ok();
        Some((
            number(parts.next()?)?,
            parts.next().and_then(number).unwrap_or(0),
        ))
    });
    match major_minor {
        Some(version) if version >= (13, 3) => Firmware::From13_3,
        _ => Firmware::Before13_3,
    }
}

fn probe(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let tree = probe
        .device_tree()
        .ok_or(ProbeError::Refused("no device tree"))?;
    let node = fdt::enabled_compatible(tree, b"apple,dcp")
        .ok_or(ProbeError::Refused("no DCP in the device tree"))?;
    let subsystem = fdt::enabled_compatible(tree, DISPLAY_SUBSYSTEM)
        .ok_or(ProbeError::Refused("no display pipe in the device tree"))?;
    let coprocessor = map_registers(probe, &node, COPROCESSOR_BYTES)?;
    let (mailbox_phandle, _) =
        reference(node.mboxes).ok_or(ProbeError::Refused("the DCP names no mailbox"))?;
    let mailbox_node = fdt::node_with_phandle(tree, mailbox_phandle).ok_or(ProbeError::Refused(
        "the DCP's mailbox is not in the device tree",
    ))?;
    let layout = mailbox::Layout::of_compatible(mailbox_node.compatible_strings()).ok_or(
        ProbeError::Refused("a mailbox of a kind EggOS does not know"),
    )?;
    let mailbox = Mailbox::new(map_registers(probe, &mailbox_node, MAILBOX_BYTES)?, layout);
    let dcp_dart = dart_of(probe, tree, &node)?;
    let mut display_dart = dart_of(probe, tree, &subsystem)?;
    let firmware = firmware_of(tree);
    let control = coprocessor.read32(CPU_CONTROL);
    coprocessor.write32(CPU_CONTROL, control | CPU_START);
    let mut rtkit = Rtkit::new(mailbox);
    let mut memory = DcpMemory {
        dart: dcp_dart,
        regions: Vec::new(),
    };
    let refused = |_: EpicError| ProbeError::Refused("the DCP did not answer as expected");
    let rtkit_refused = |_: RtkitError| ProbeError::Refused("the DCP's RTKit did not start");
    rtkit.wake().map_err(rtkit_refused)?;
    let mut link = DcpLink {
        rtkit: &mut rtkit,
        memory: &mut memory,
        waiting: VecDeque::new(),
        deadline: probe.deadline(BOOT_TIMEOUT_MS),
    };
    link.until_running().map_err(rtkit_refused)?;
    link.rtkit
        .start_endpoint(dcp::ENDPOINT)
        .map_err(rtkit_refused)?;
    let mut service = DisplayService::start(&mut link, firmware).map_err(refused)?;
    service.set_power(&mut link, true).map_err(refused)?;
    if service.hot_plug(&mut link).map_err(refused)?.is_none() {
        return Err(ProbeError::Refused("no screen is attached"));
    }
    let timing = dcp::choose_timing(&service.timing_modes(&mut link).map_err(refused)?)
        .ok_or(ProbeError::Refused("the screen reports no timing mode"))?;
    let color = dcp::choose_color(&service.color_modes(&mut link).map_err(refused)?)
        .ok_or(ProbeError::Refused("the screen reports no color mode"))?;
    service
        .set_mode(&mut link, timing, color)
        .map_err(refused)?;
    let stride = timing.width * BYTES_PER_PIXEL;
    let bytes = (u64::from(stride) * u64::from(timing.height)).next_multiple_of(PAGE_BYTES);
    let framebuffer = DmaRegion::allocate(bytes as usize)
        .map_err(|_| ProbeError::Refused("no memory for the framebuffer"))?;
    let physical = framebuffer.physical();
    let address = link
        .memory
        .dart
        .find_free(FIRST_SHARED_ADDRESS, bytes)
        .ok_or(ProbeError::Refused("no room in the DCP's DART"))?;
    let shared_refused = |_| ProbeError::Refused("the framebuffer could not be mapped");
    link.memory
        .dart
        .map(address, physical, bytes)
        .map_err(shared_refused)?;
    display_dart
        .map(address, physical, bytes)
        .map_err(shared_refused)?;
    let surface = Surface {
        address,
        stride,
        width: timing.width,
        height: timing.height,
        format: surface_format::BGRA,
    };
    service.set_surface(&mut link, &surface).map_err(refused)?;
    let pixels = probe.map_write_combining(PhysicalRange::from_start_length(physical, bytes))?;
    let mode = Mode::new(timing.width, timing.height);
    let scanout = AppleScanout {
        pixels: pixels as *mut u32,
        mode,
    };
    core::mem::forget(framebuffer);
    core::mem::forget(memory);
    core::mem::forget(display_dart);
    if crate::display::install(scanout).is_err() {
        return Err(ProbeError::Refused("a display is already in use"));
    }
    if let Some(node) = probe.table().get_mut(id) {
        node.delivery = super::Delivery::Polled;
    }
    Ok(())
}
