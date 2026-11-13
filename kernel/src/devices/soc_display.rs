//! The display controllers beside Arm Mali GPUs (D548), Rockchip's VOP on the RK3399 and
//! Allwinner's DE2 mixers on the H3, H5 and A64, and Qualcomm's MDP5 beside the Adreno GPUs of
//! the Snapdragon 400 to 820 (D549). Each takes over the plane the firmware lit,
//! at its mode, and shows the desktop through two framebuffers in turn
//! ([`crate::display::flipping`]): the firmware's and one of the kernel's own.

use super::dma::DmaRegion;
use super::{DeviceId, DriverDescriptor, Match, Probe, ProbeError, ProbePass};
use crate::display::Mode;
use crate::display::flipping::{Buffer, FlipEngine, FlippingScanout};
use eggos_drivers::gpu::allwinner_de2::{self, Mixer};
use eggos_drivers::gpu::qualcomm_mdp5::{self, Mdp5, Pipe};
use eggos_drivers::gpu::rockchip_vop::{self, Vop};
use eggos_drivers::io::Mmio;
use eggos_hal::PhysicalRange;

pub static ROCKCHIP_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "rockchip-vop",
    pass: ProbePass::Device,
    matches: &[
        Match::Compatible(rockchip_vop::COMPATIBLE[0]),
        Match::Compatible(rockchip_vop::COMPATIBLE[1]),
    ],
    probe: probe_rockchip,
};

pub static ALLWINNER_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "allwinner-de2",
    pass: ProbePass::Device,
    matches: &[
        Match::Compatible(allwinner_de2::COMPATIBLE[0]),
        Match::Compatible(allwinner_de2::COMPATIBLE[1]),
        Match::Compatible(allwinner_de2::COMPATIBLE[2]),
    ],
    probe: probe_allwinner,
};

pub static QUALCOMM_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "qualcomm-mdp5",
    pass: ProbePass::Device,
    matches: &[Match::Compatible(qualcomm_mdp5::COMPATIBLE)],
    probe: probe_qualcomm,
};

const FOUR_GIGABYTES: u64 = 1 << 32;

struct VopEngine(Vop<Mmio>);

impl FlipEngine for VopEngine {
    fn flip(&mut self, address: u64) {
        self.0.flip(address);
    }

    fn flipped(&mut self) -> bool {
        self.0.flipped()
    }
}

struct MixerEngine {
    mixer: Mixer<Mmio>,
    channel: usize,
}

impl FlipEngine for MixerEngine {
    fn flip(&mut self, address: u64) {
        self.mixer.flip(self.channel, address);
    }

    fn flipped(&mut self) -> bool {
        self.mixer.flipped()
    }
}

struct Mdp5Engine {
    mdp: Mdp5<Mmio>,
    pipe: Pipe,
}

impl FlipEngine for Mdp5Engine {
    fn flip(&mut self, address: u64) {
        self.mdp.flip(self.pipe, address);
    }

    fn flipped(&mut self) -> bool {
        self.mdp.flipped(self.pipe)
    }
}

fn registers(probe: &mut dyn Probe, id: DeviceId) -> Result<Mmio, ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let region = node.regions().next().ok_or(ProbeError::Refused(
        "the display controller has no registers",
    ))?;
    let virt = probe.map_window(region)?;
    // SAFETY: `map_window` mapped the controller's register block as device memory for good;
    // only this driver reaches it.
    Ok(unsafe { Mmio::new(virt, region.len() as usize) })
}

fn take_over<E: FlipEngine + 'static>(
    probe: &mut dyn Probe,
    engine: E,
    (address, width, height, stride): (u64, u32, u32, u32),
    below_4_gib: bool,
) -> Result<(), ProbeError> {
    let bytes = u64::from(stride) * u64::from(height);
    if stride < width * 4 || bytes == 0 {
        return Err(ProbeError::Refused(
            "the lit plane's rows are shorter than its width",
        ));
    }
    let firmware = probe.map_write_combining(PhysicalRange::from_start_length(address, bytes))?;
    let second = DmaRegion::allocate(bytes as usize)
        .map_err(|_| ProbeError::Refused("no memory for a second framebuffer"))?;
    let physical = second.physical();
    if below_4_gib && physical + bytes > FOUR_GIGABYTES {
        return Err(ProbeError::Refused(
            "the second framebuffer lies past 4 GiB",
        ));
    }
    let own = probe.map_write_combining(PhysicalRange::from_start_length(physical, bytes))?;
    let _ = second;
    let buffers = [
        Buffer {
            address,
            window: firmware,
        },
        Buffer {
            address: physical,
            window: own,
        },
    ];
    let scanout = FlippingScanout::new(engine, Mode::new(width, height), stride as usize, buffers)
        .ok_or(ProbeError::Refused("no memory for the frame"))?;
    crate::display::install(scanout).map_err(|_| ProbeError::Refused("a display is already in use"))
}

fn probe_rockchip(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let vop = Vop::new(registers(probe, id)?);
    let lit = vop
        .lit_window()
        .ok_or(ProbeError::Refused("the firmware lit no 32-bit window"))?;
    let plane = (lit.address, lit.width, lit.height, lit.stride);
    take_over(probe, VopEngine(vop), plane, true)
}

fn probe_allwinner(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let mixer = Mixer::new(registers(probe, id)?);
    let lit = mixer
        .lit_layer()
        .ok_or(ProbeError::Refused("the firmware lit no UI channel"))?;
    let plane = (lit.address, lit.width, lit.height, lit.stride);
    let engine = MixerEngine {
        mixer,
        channel: lit.channel,
    };
    take_over(probe, engine, plane, false)
}

fn probe_qualcomm(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let mdp = Mdp5::new(registers(probe, id)?)
        .ok_or(ProbeError::Refused("the display processor is not an MDP5"))?;
    let lit = mdp
        .lit_pipe()
        .ok_or(ProbeError::Refused("the bootloader lit no 32-bit pipe"))?;
    let plane = (lit.address, lit.width, lit.height, lit.stride);
    let engine = Mdp5Engine {
        mdp,
        pipe: lit.pipe,
    };
    take_over(probe, engine, plane, true)
}
