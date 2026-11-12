//! AMD's and NVIDIA's graphics, part 1 (D448, D449): at probe the card's video BIOS is read
//! from its expansion ROM and its tables parsed (AMD's ATOM tables,
//! [`eggos_drivers::gpu::amd_atom`]; NVIDIA's BIT and DCB, [`eggos_drivers::gpu::nvidia_bios`]),
//! so `/proc/driver/display` says which outputs the board has and which firmware it carries.
//! The firmware's framebuffer (D236) stays the display until the display engines are driven.

use super::{DeviceId, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::sync::SpinLock;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_drivers::gpu::amd_atom::AtomBios;
use eggos_drivers::gpu::nvidia_bios::NvidiaBios;
use eggos_hal::PhysicalRange;

const VENDOR_AMD: u16 = 0x1002;
const VENDOR_NVIDIA: u16 = 0x10de;
const DISPLAY_CONTROLLER: u8 = 0x03;
const EXPANSION_ROM: u16 = 0x30;
const ROM_ENABLE: u32 = 1;
const ROM_ADDRESS_MASK: u32 = 0xffff_f800;
const ROM_BYTES: usize = 128 << 10;

static SUMMARY: SpinLock<String> = SpinLock::new(String::new());

/// AMD's GPUs (PCI vendor 1002h), ahead of the display class's other drivers.
pub static AMD_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "amd-display",
    pass: ProbePass::Device,
    matches: &[Match::PciIdRange {
        vendor: VENDOR_AMD,
        first: 0,
        last: 0xffff,
    }],
    probe: probe_amd_display,
};

/// NVIDIA's GPUs (PCI vendor 10DEh), ahead of the display class's other drivers.
pub static NVIDIA_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "nvidia-display",
    pass: ProbePass::Device,
    matches: &[Match::PciIdRange {
        vendor: VENDOR_NVIDIA,
        first: 0,
        last: 0xffff,
    }],
    probe: probe_nvidia_display,
};

fn probe_nvidia_display(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe.table().get(id).ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if function.class != DISPLAY_CONTROLLER {
        return Err(ProbeError::Refused("not a display controller"));
    }
    let image = read_rom(probe, function.address)?;
    let bios = NvidiaBios::parse(&image).ok_or(ProbeError::Refused("the expansion ROM holds no video BIOS"))?;
    let mut line = String::new();
    let _ = write!(line, "nvidia {:04x}", function.device_id);
    if let Some(version) = bios.version {
        let _ = write!(line, " bios {version:08x}");
    }
    for output in bios.outputs.iter().flatten() {
        let _ = write!(line, " {}", output.name());
    }
    line.push('\n');
    SUMMARY.lock().push_str(&line);
    Ok(())
}

/// What the AMD and NVIDIA boards' video BIOSes said, one line each; empty without one.
pub fn summary() -> String {
    SUMMARY.lock().clone()
}

fn read_rom(probe: &mut dyn Probe, address: eggos_drivers::pci::Address) -> Result<Vec<u8>, ProbeError> {
    let config = probe.pci_config().ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let rom = config.read32(address, EXPANSION_ROM);
    let base = rom & ROM_ADDRESS_MASK;
    if base == 0 {
        return Err(ProbeError::Refused("the expansion ROM has no address"));
    }
    config.write32(address, EXPANSION_ROM, rom | ROM_ENABLE);
    let window = probe.map_window(PhysicalRange::from_start_length(u64::from(base), ROM_BYTES as u64));
    let image = window.map(|start| {
        let mut image = Vec::with_capacity(ROM_BYTES);
        for offset in 0..ROM_BYTES {
            // SAFETY: `map_window` mapped the ROM's `ROM_BYTES` as device memory for good; the
            // ROM is read-only and only this probe reads it.
            image.push(unsafe { core::ptr::read_volatile((start + offset) as *const u8) });
        }
        image
    });
    if let Some(config) = probe.pci_config() {
        config.write32(address, EXPANSION_ROM, rom & !ROM_ENABLE);
    }
    image
}

fn probe_amd_display(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe.table().get(id).ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if function.class != DISPLAY_CONTROLLER {
        return Err(ProbeError::Refused("not a display controller"));
    }
    let image = read_rom(probe, function.address)?;
    let bios = AtomBios::parse(&image).ok_or(ProbeError::Refused("the expansion ROM holds no ATOM BIOS"))?;
    let mut line = String::new();
    let _ = write!(line, "amd {:04x}", function.device_id);
    if let Some(revision) = bios.firmware_revision {
        let _ = write!(line, " firmware {revision:08x}");
    }
    if let Some(clock) = bios.boot_engine_khz {
        let _ = write!(line, " engine {} MHz", clock / 1000);
    }
    for path in bios.paths.iter().flatten() {
        let _ = write!(line, " {}", path.connector.name());
    }
    line.push('\n');
    SUMMARY.lock().push_str(&line);
    Ok(())
}
