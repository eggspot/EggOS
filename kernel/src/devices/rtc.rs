//! The device-tree real-time clocks (D150): Arm's PL031 (QEMU's AArch64 `virt`) and Google's
//! Goldfish clock (QEMU's RISC-V `virt`). Probing maps the clock's registers and registers it
//! as the machine's real-time clock ([`crate::wall_clock::register`]), which reads it once at
//! boot; a second clock is refused before its registers are mapped. The PC's CMOS clock is the platform's own ([`eggos_hal::Platform::platform_clock`]).

use super::{DeviceId, DriverDescriptor, Match, Probe, ProbeError, ProbePass};
use alloc::boxed::Box;
use eggos_drivers::io::Mmio;
use eggos_drivers::rtc::{Goldfish, Pl031, RealTimeClock, goldfish, pl031};

/// Arm's PL031.
pub static PL031_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "rtc-pl031",
    pass: ProbePass::Device,
    matches: &[Match::Compatible(pl031::COMPATIBLE)],
    probe: probe_pl031,
};

/// Google's Goldfish real-time clock.
pub static GOLDFISH_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "rtc-goldfish",
    pass: ProbePass::Device,
    matches: &[Match::Compatible(goldfish::COMPATIBLE)],
    probe: probe_goldfish,
};

const ANOTHER_CLOCK_IN_USE: ProbeError = ProbeError::Refused("another real-time clock is in use");

fn registers(probe: &mut dyn Probe, id: DeviceId, size: usize) -> Result<Mmio, ProbeError> {
    if crate::wall_clock::has_clock() {
        return Err(ANOTHER_CLOCK_IN_USE);
    }
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let region = node
        .regions()
        .next()
        .ok_or(ProbeError::Refused("the clock has no registers"))?;
    if region.len() < size as u64 {
        return Err(ProbeError::Refused(
            "the clock's registers are shorter than its driver needs",
        ));
    }
    let base = probe.map_window(region)?;
    // SAFETY: `map_window` mapped the clock's registers as device memory, for good, and only
    // this driver reaches them, through the wall clock's lock.
    Ok(unsafe { Mmio::new(base, size) })
}

fn install(clock: Box<dyn RealTimeClock + Send>) -> Result<(), ProbeError> {
    match crate::wall_clock::register(clock) {
        true => Ok(()),
        false => Err(ANOTHER_CLOCK_IN_USE),
    }
}

fn probe_pl031(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let clock = Pl031::new(registers(probe, id, pl031::REGISTER_BLOCK_SIZE)?);
    clock.start();
    install(Box::new(clock))
}

fn probe_goldfish(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let clock = Goldfish::new(registers(probe, id, goldfish::REGISTER_BLOCK_SIZE)?);
    install(Box::new(clock))
}
