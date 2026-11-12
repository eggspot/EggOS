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
use eggos_drivers::gpu::amd_atom_interpreter::Interpreter;
use eggos_drivers::gpu::amd_dcn;
use eggos_drivers::gpu::amd_discovery::{OFFSET_FROM_TOP, display_block, dmcub_file_name};
use eggos_drivers::gpu::nvidia_bios::NvidiaBios;
use eggos_drivers::gpu::{nvidia_falcon, nvidia_fwsec};
use eggos_drivers::io::RegisterIo;
use eggos_drivers::gpu::nvidia_gsp::{self, Chip};
use eggos_drivers::gpu::nvidia_gsp_boot::{self, SharedMemory, SystemImages, WprLayout};
use eggos_drivers::gpu::nvidia_gsp_display;
use eggos_drivers::gpu::nvidia_gsp_queue::{self, Queues, SequencerStep, SystemInfo};
use eggos_drivers::dma::DmaMemory;
use super::dma::DmaRegion;
use eggos_hal::PhysicalRange;

const VENDOR_AMD: u16 = 0x1002;
const VENDOR_NVIDIA: u16 = 0x10de;
const DISPLAY_CONTROLLER: u8 = 0x03;
const EXPANSION_ROM: u16 = 0x30;
const ROM_ENABLE: u32 = 1;
const ROM_ADDRESS_MASK: u32 = 0xffff_f800;
const ROM_BYTES: usize = 128 << 10;
const REGISTER_BAR: usize = 5;

static SUMMARY: SpinLock<String> = SpinLock::new(String::new());

/// AMD's GPUs (PCI vendor 1002h), ahead of the display class's other drivers.
const ATOM_COMMAND_TABLES: usize = 81;
const BOOT_0_PAGE: u64 = 4096;
const GSP_BLOCK_BYTES: u64 = 0x4000;
const PROM_START: u64 = 0x30_0000;
const USABLE_VIDEO_MEMORY_MB: usize = 0x11_83a4;
const VBIOS_SCRATCH_FRTS: usize = 0x1400 + 4 * 0x0e;
const WPR2_LOW: usize = 0x1f_a824;
const WPR2_HIGH: usize = 0x1f_a828;
const FRTS_BYTES: u64 = 1 << 20;
const AMPERE: u16 = 0x17;
const GSP_FIRMWARE_FOLDER: &str = "/usr/lib/firmware/nvidia";
const LARGEST_GSP_FILE: u64 = 64 << 20;
const GSP_FILE_RETRY: core::time::Duration = core::time::Duration::from_secs(10);
const GSP_POLLS: u32 = 50_000_000;
const PAGED_CHUNK_BYTES: usize = 4 << 20;
const CONFIG_MIRROR: (u32, u32) = (0x8_8000, 0x1000);
const USER_ADDRESS_TOP: u64 = 0x7fff_ffff_f000;
const FRAMEBUFFER_BAR: usize = 1;
const INSTANCE_MEMORY_BAR: usize = 3;

struct GspTarget {
    registers: usize,
    length: usize,
    chip: &'static str,
    family: &'static str,
    fb_bytes: u64,
    bios_start: u64,
    system: SystemInfo,
}

static GSP_TARGET: SpinLock<Option<GspTarget>> = SpinLock::new(None);
static GSP_MEMORY: SpinLock<Vec<DmaRegion>> = SpinLock::new(Vec::new());
const FWSEC_MS: u64 = 1000;
const PROM_BYTES: u64 = 0x10_0000;

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
    if let Some(config) = probe.pci_config() {
        eggos_drivers::pci::enable_memory_and_bus_master(config, function.address);
    }
    if let Some((imem, dmem)) = read_gsp_memories(probe, node.regions[0]) {
        let _ = write!(line, " gsp falcon {} KiB imem {} KiB dmem", imem >> 10, dmem >> 10);
    }
    let mut wpr2_ready = false;
    if let Some(rom) = read_prom(probe, node.regions[0])
        && let Some(fwsec) = nvidia_fwsec::find(&rom, false)
    {
        let _ = write!(
            line,
            " fwsec {} + {} bytes, signature versions {:#06b}",
            fwsec.imem_bytes, fwsec.dmem_bytes, fwsec.signature_versions
        );
        match run_fwsec(probe, node.regions[0], &fwsec) {
            Ok(region) => {
                wpr2_ready = true;
                let _ = write!(line, " wpr2 at {:#x}", region.start);
            }
            Err(reason) => {
                let _ = write!(line, " no wpr2: {reason}");
            }
        }
    }
    if let Some(chip) = read_boot_0(probe, node.regions[0]).map(Chip::of_boot_0) {
        match (chip.name(), chip.signature_family()) {
            (Some(name), Some(family)) if chip.runs_gsp() => {
                let _ = write!(line, " chip {name} gsp nvidia/{name}/gsp ({family})");
                if wpr2_ready && chip.architecture() >= AMPERE {
                    let a = function.address;
                    let system = SystemInfo {
                        registers: node.regions[0].start,
                        framebuffer: node.regions[FRAMEBUFFER_BAR].start,
                        instance_memory: node.regions[INSTANCE_MEMORY_BAR].start,
                        pci_id: u64::from(a.bus) << 8
                            | u64::from(a.device) << 3
                            | u64::from(a.function),
                        user_address_top: USER_ADDRESS_TOP,
                        config_mirror: CONFIG_MIRROR,
                    };
                    match keep_gsp_target(probe, node.regions[0], (name, family), system) {
                        Ok(()) => line.push_str(", gsp-rm boots once its firmware is there"),
                        Err(reason) => {
                            let _ = write!(line, ", gsp-rm cannot boot: {reason}");
                        }
                    }
                }
            }
            _ => {
                let _ = write!(line, " chip {:03x}", chip.id);
            }
        }
    }
    line.push('\n');
    SUMMARY.lock().push_str(&line);
    Ok(())
}

/// What the AMD and NVIDIA boards' video BIOSes said, one line each; empty without one.
pub fn summary() -> String {
    SUMMARY.lock().clone()
}

fn read_boot_0(probe: &mut dyn Probe, registers: PhysicalRange) -> Option<u32> {
    if registers.len() < BOOT_0_PAGE {
        return None;
    }
    let start = probe
        .map_window(PhysicalRange::from_start_length(registers.start, BOOT_0_PAGE))
        .ok()?;
    // SAFETY: `map_window` mapped BAR 0's first page, the GPU's registers, as device memory
    // for good; `PMC_BOOT_0` is a read-only identification register.
    Some(unsafe { core::ptr::read_volatile((start + nvidia_gsp::BOOT_0) as *const u32) })
}

/// The usable video memory's bytes and where the VGA workspace starts in it.
fn video_memory(regs: &eggos_drivers::io::Mmio) -> (u64, u64) {
    let bytes = u64::from(regs.read32(USABLE_VIDEO_MEMORY_MB)) << 20;
    let workspace = regs.read32(nvidia_gsp_boot::VGA_WORKSPACE_REGISTER);
    (bytes, nvidia_gsp_boot::bios_start(bytes, workspace))
}

/// Keeps BAR 0 mapped and what the GSP boot needs to know about the board until the file
/// systems hold its firmware (D522).
fn keep_gsp_target(
    probe: &mut dyn Probe,
    registers: PhysicalRange,
    (chip, family): (&'static str, &'static str),
    system: SystemInfo,
) -> Result<(), &'static str> {
    let length = usize::try_from(registers.len()).map_err(|_| "BAR 0 is too large")?;
    let base = probe
        .map_window(registers)
        .map_err(|_| "BAR 0 could not be mapped")?;
    // SAFETY: `map_window` mapped BAR 0, `length` bytes, as device memory for good; only this
    // driver drives the GPU's registers.
    let regs = unsafe { eggos_drivers::io::Mmio::new(base, length) };
    let (fb_bytes, bios_start) = video_memory(&regs);
    *GSP_TARGET.lock() = Some(GspTarget {
        registers: base,
        length,
        chip,
        family,
        fb_bytes,
        bios_start,
        system,
    });
    Ok(())
}

fn polls() -> impl FnMut() -> bool {
    let mut left = GSP_POLLS;
    move || {
        left = left.saturating_sub(1);
        left > 0
    }
}

fn device_address(region: &DmaRegion) -> u64 {
    DmaMemory::dma_addr(region).get()
}

fn dma_bytes(region: &mut DmaRegion) -> &mut [u8] {
    // SAFETY: the region maps `len()` bytes for the CPU; `&mut` makes the borrow unique, and no
    // device reaches the region before GSP boots from it.
    unsafe {
        core::slice::from_raw_parts_mut(DmaMemory::as_mut_ptr(region), DmaMemory::len(region))
    }
}

fn allocate(bytes: usize) -> Result<DmaRegion, &'static str> {
    DmaRegion::allocate(bytes).map_err(|_| "no memory for GSP-RM")
}

fn dma_copy(bytes: &[u8]) -> Result<DmaRegion, &'static str> {
    let mut region = allocate(bytes.len())?;
    dma_bytes(&mut region)[..bytes.len()].copy_from_slice(bytes);
    Ok(region)
}

fn paged_copy(bytes: &[u8], kept: &mut Vec<DmaRegion>) -> Result<Vec<u64>, &'static str> {
    let mut pages = Vec::new();
    for chunk in bytes.chunks(PAGED_CHUNK_BYTES) {
        let region = dma_copy(chunk)?;
        let start = device_address(&region);
        let count = (chunk.len() as u64).div_ceil(nvidia_gsp_boot::PAGE_BYTES);
        pages.extend((0..count).map(|page| start + page * nvidia_gsp_boot::PAGE_BYTES));
        kept.push(region);
    }
    Ok(pages)
}

/// Boots GSP-RM (D522): its image, boot binary and signature into system memory, the WPR
/// metadata, libos's regions and the queues' shared memory; GSP's Falcon reset into RISC-V
/// with libos's arguments in its mailboxes; SEC2's booter, signed for the fuse, run with the
/// metadata's address, which copies GSP-RM into WPR2 and starts it; then the boot binary's
/// version told and the RISC-V core seen running. The memory stays GSP-RM's for good. The
/// system information and the registry wait in the command queue before the booter runs
/// (D523); GSP-RM's messages are then read, its CPU sequencer requests run (`delay` waits a
/// number of microseconds), until it says it finished starting. Then the screens GSP-RM finds
/// are asked for (D524); a summary of them.
fn boot_gsp(
    target: &GspTarget,
    (gsp, boot_file, booter_file): (&[u8], &[u8], &[u8]),
    delay: &mut dyn FnMut(u32),
) -> Result<String, &'static str> {
    let firmware = nvidia_gsp::gsp_firmware(gsp, target.family).ok_or("not a GSP firmware file")?;
    let (image_at, image_bytes) = firmware.image;
    let (signature_at, signature_bytes) = firmware
        .signature
        .ok_or("the GSP firmware has no signature for this chip")?;
    let boot = nvidia_gsp_boot::bootloader(boot_file).ok_or("not a GSP boot binary")?;
    let (boot_at, boot_bytes) = boot.data;
    let sizes = (boot_bytes as u64, image_bytes as u64);
    let layout = WprLayout::new(target.fb_bytes, target.bios_start, sizes.0, sizes.1)
        .ok_or("GSP-RM does not fit the video memory")?;
    let mut kept = Vec::new();
    let pages = paged_copy(&gsp[image_at..image_at + image_bytes], &mut kept)?;
    let table_bytes =
        nvidia_gsp_boot::radix3_bytes(image_bytes as u64).ok_or("the GSP image is too large")?;
    let mut radix3 = allocate(table_bytes as usize)?;
    let radix3_address = device_address(&radix3);
    nvidia_gsp_boot::write_radix3(dma_bytes(&mut radix3), radix3_address, &pages)
        .ok_or("radix3 does not fit")?;
    let bootloader = dma_copy(&boot_file[boot_at..boot_at + boot_bytes])?;
    let signature = dma_copy(&gsp[signature_at..signature_at + signature_bytes])?;
    let images = SystemImages {
        radix3: radix3_address,
        bootloader: device_address(&bootloader),
        signature: (device_address(&signature), signature_bytes as u64),
    };
    let meta = dma_copy(&nvidia_gsp_boot::wpr_meta(&layout, &images, &boot))?;
    let mut regions = [(0u64, 0u64); 4];
    for slot in regions.iter_mut().take(3) {
        let mut log = allocate(nvidia_gsp_boot::LOG_BYTES)?;
        let address = device_address(&log);
        let bytes = &mut dma_bytes(&mut log)[..nvidia_gsp_boot::LOG_BYTES];
        nvidia_gsp_boot::write_log_pages(bytes, address);
        *slot = (address, nvidia_gsp_boot::LOG_BYTES as u64);
        kept.push(log);
    }
    let shared = SharedMemory::new();
    let mut queues = allocate(shared.bytes as usize)?;
    let queues_address = device_address(&queues);
    shared.initialize(dma_bytes(&mut queues), queues_address);
    // SAFETY: the region maps `len()` bytes for the CPU for good; GSP-RM reaches it too, which
    // the queues' volatile accesses and pointer protocol allow for.
    let queue_memory = unsafe {
        let start = DmaMemory::as_mut_ptr(&queues) as usize;
        eggos_drivers::io::Mmio::new(start, DmaMemory::len(&queues))
    };
    let mut channel = Queues::new(queue_memory, shared);
    let system = nvidia_gsp_queue::system_info(&target.system);
    channel
        .send(nvidia_gsp_queue::function::SET_SYSTEM_INFO, &system)
        .map_err(nvidia_gsp_queue::QueueError::description)?;
    channel
        .send(
            nvidia_gsp_queue::function::SET_REGISTRY,
            &nvidia_gsp_queue::registry(&nvidia_gsp_queue::DEFAULT_REGISTRY),
        )
        .map_err(nvidia_gsp_queue::QueueError::description)?;
    let rm_arguments = dma_copy(&shared.rm_arguments(queues_address))?;
    regions[3] = (device_address(&rm_arguments), nvidia_gsp_boot::PAGE_BYTES);
    let libos = dma_copy(&nvidia_gsp_boot::libos_arguments(&regions))?;
    // SAFETY: `keep_gsp_target` mapped BAR 0, `length` bytes, as device memory for good, and
    // only this thread drives the GPU's registers now.
    let regs = unsafe { eggos_drivers::io::Mmio::new(target.registers, target.length) };
    // SAFETY: as above, for GSP's Falcon block.
    let gsp_regs = unsafe { eggos_drivers::io::Mmio::new(target.registers, target.length) };
    // SAFETY: as above, for SEC2's Falcon block.
    let sec2_regs = unsafe { eggos_drivers::io::Mmio::new(target.registers, target.length) };
    let gsp_falcon = nvidia_falcon::Falcon::new(gsp_regs, nvidia_falcon::GSP_BASE);
    gsp_falcon
        .reset(&mut polls())
        .map_err(nvidia_falcon::FalconError::description)?;
    gsp_falcon.prepare_riscv(device_address(&libos));
    let (engines, ucode) =
        nvidia_gsp_boot::booter_identity(booter_file).ok_or("not a booter file")?;
    let fuse = nvidia_gsp_boot::booter_fuse(engines, ucode).map_or(0, |at| regs.read32(at));
    let booter = nvidia_gsp_boot::booter(booter_file, fuse)
        .ok_or("the booter has no signature for the fuse")?;
    let booter_image = dma_copy(&booter.image)?;
    let sec2 = nvidia_falcon::Falcon::new(sec2_regs, nvidia_falcon::SEC2_BASE);
    sec2.reset(&mut polls())
        .map_err(nvidia_falcon::FalconError::description)?;
    let program = nvidia_falcon::SecureProgram {
        image: device_address(&booter_image),
        imem_physical: 0,
        imem_virtual: booter.code_virtual,
        imem_bytes: booter.code_bytes,
        dmem_physical: 0,
        dmem_bytes: booter.data_bytes,
        signature_place: booter.signature_place,
        engine_mask: booter.engine_mask,
        ucode_id: booter.ucode_id,
    };
    let meta_address = device_address(&meta);
    let mailboxes = (meta_address as u32, (meta_address >> 32) as u32);
    let (status, _) = sec2
        .run_secure(nvidia_falcon::SEC2_FBIF, &program, mailboxes, &mut polls())
        .map_err(nvidia_falcon::FalconError::description)?;
    kept.extend([radix3, bootloader, signature, meta, queues, rm_arguments, libos, booter_image]);
    GSP_MEMORY.lock().append(&mut kept);
    if status != 0 {
        return Err("the booter reported an error");
    }
    gsp_falcon.set_os_version(boot.app_version);
    gsp_falcon.announce_command();
    let mut keep_waiting = polls();
    while !gsp_falcon.riscv_active() {
        if !keep_waiting() {
            return Err("GSP's RISC-V core did not start");
        }
        core::hint::spin_loop();
    }
    let mut gsp_rm = GspRm {
        channel,
        regs,
        length: target.length,
        falcon: gsp_falcon,
    };
    gsp_rm.await_message(nvidia_gsp_queue::event::INIT_DONE, delay)?;
    let sinks = nvidia_gsp_display::find_sinks(&mut |function, payload| {
        gsp_rm.call(function, payload, delay)
    })
    .map_err(|error| match error {
        nvidia_gsp_display::DisplayError::Call(reason) => reason,
        nvidia_gsp_display::DisplayError::Refused(_) => "GSP-RM refused a display call",
        nvidia_gsp_display::DisplayError::ShortAnswer => "GSP-RM's display answer was short",
    })?;
    let mut line = String::new();
    for sink in sinks {
        let _ = write!(line, " display {}", sink.display);
        if let Some(edid) = sink.edid.as_deref().and_then(eggos_drivers::edid::Edid::parse) {
            let maker = core::str::from_utf8(&edid.maker).unwrap_or("?");
            let _ = write!(line, " {maker} {}", edid.name());
            if let Some(mode) = edid.preferred {
                let _ = write!(line, " preferred {}x{}", mode.width, mode.height);
            }
        }
    }
    Ok(line)
}

/// GSP-RM once it runs: its queues, BAR 0 (`length` bytes) and GSP's Falcon.
struct GspRm {
    channel: Queues<eggos_drivers::io::Mmio>,
    regs: eggos_drivers::io::Mmio,
    length: usize,
    falcon: nvidia_falcon::Falcon<eggos_drivers::io::Mmio>,
}

impl GspRm {
    /// Reads GSP-RM's messages until one of `function` comes, running its CPU sequencer
    /// requests on the way (D523); that message.
    fn await_message(
        &mut self,
        function: u32,
        delay: &mut dyn FnMut(u32),
    ) -> Result<nvidia_gsp_queue::Message, &'static str> {
        let mut keep_waiting = polls();
        loop {
            let message = self
                .channel
                .receive()
                .map_err(nvidia_gsp_queue::QueueError::description)?;
            let Some(message) = message else {
                if !keep_waiting() {
                    return Err("GSP-RM did not answer in time");
                }
                core::hint::spin_loop();
                continue;
            };
            match message.function {
                found if found == function => return Ok(message),
                nvidia_gsp_queue::event::RUN_CPU_SEQUENCER => {
                    let steps = nvidia_gsp_queue::sequencer_steps(&message.payload)
                        .ok_or("GSP-RM sent an unknown sequencer operation")?;
                    run_sequencer(&self.regs, self.length, &self.falcon, &steps, delay)?;
                }
                nvidia_gsp_queue::event::OS_ERROR_LOG => return Err("GSP-RM reported an error"),
                _ => {}
            }
        }
    }

    /// Sends remote call `function` with `payload` and waits for its answer's payload (D524).
    fn call(
        &mut self,
        function: u32,
        payload: &[u8],
        delay: &mut dyn FnMut(u32),
    ) -> Result<Vec<u8>, &'static str> {
        self.channel
            .send(function, payload)
            .map_err(nvidia_gsp_queue::QueueError::description)?;
        self.falcon.announce_command();
        Ok(self.await_message(function, delay)?.payload)
    }
}

/// Runs GSP-RM's CPU sequencer `steps` on BAR 0 (`length` bytes, behind `regs`) and GSP's
/// Falcon (D523).
fn run_sequencer(
    regs: &eggos_drivers::io::Mmio,
    length: usize,
    gsp: &nvidia_falcon::Falcon<eggos_drivers::io::Mmio>,
    steps: &[SequencerStep],
    delay: &mut dyn FnMut(u32),
) -> Result<(), &'static str> {
    let place = |address: u32| {
        let address = address as usize;
        (address % 4 == 0 && address + 4 <= length)
            .then_some(address)
            .ok_or("GSP-RM's sequencer named a register outside BAR 0")
    };
    for step in steps {
        match *step {
            SequencerStep::Write { address, value } => regs.write32(place(address)?, value),
            SequencerStep::Modify { address, mask, value } => {
                let at = place(address)?;
                regs.write32(at, regs.read32(at) & !mask | value);
            }
            SequencerStep::Poll { address, mask, value, .. } => {
                let at = place(address)?;
                let mut keep_waiting = polls();
                while regs.read32(at) & mask != value {
                    if !keep_waiting() {
                        return Err("a register GSP-RM's sequencer waits for never changed");
                    }
                    core::hint::spin_loop();
                }
            }
            SequencerStep::Delay { microseconds } => delay(microseconds),
            SequencerStep::Store { address, .. } => {
                place(address)?;
            }
            SequencerStep::CoreReset => gsp
                .reset(&mut polls())
                .map_err(nvidia_falcon::FalconError::description)?,
            SequencerStep::CoreStart => gsp.start(),
            SequencerStep::CoreWaitForHalt => gsp
                .wait_for_halt(&mut polls())
                .map_err(nvidia_falcon::FalconError::description)?,
            SequencerStep::CoreResume => return Err("GSP-RM asked to resume, not written yet"),
        }
    }
    Ok(())
}

/// Starts the thread that boots GSP-RM once the `display-firmware` component's files for the
/// probed chip are there (D522); whether a board waits for it.
pub fn start_gsp<P: eggos_hal::Platform>() -> bool {
    let Some(target) = GSP_TARGET.lock().take() else {
        return false;
    };
    crate::thread::spawn::<P, _>(move || {
        let version = nvidia_gsp_boot::FIRMWARE_VERSION;
        let path = |name: &str| {
            alloc::format!("{GSP_FIRMWARE_FOLDER}/{}/gsp/{name}-{version}.bin", target.chip)
        };
        let read = |name: &str| super::read_firmware(&path(name), LARGEST_GSP_FILE);
        let (gsp, boot, booter) = loop {
            if let (Some(gsp), Some(boot), Some(booter)) =
                (read("gsp"), read("bootloader"), read("booter_load"))
            {
                break (gsp, boot, booter);
            }
            crate::thread::sleep::<P>(GSP_FILE_RETRY);
        };
        let mut delay = |microseconds: u32| {
            crate::thread::sleep::<P>(core::time::Duration::from_micros(microseconds.into()));
        };
        let line = match boot_gsp(&target, (&gsp, &boot, &booter), &mut delay) {
            Ok(sinks) => {
                alloc::format!("nvidia {} gsp-rm {version} started{sinks}\n", target.chip)
            }
            Err(reason) => {
                alloc::format!("nvidia {} gsp-rm did not start: {reason}\n", target.chip)
            }
        };
        SUMMARY.lock().push_str(&line);
    })
    .is_ok()
}

/// Runs FWSEC's FRTS command on GSP's Falcon (D513), so the protected region WPR2 is set up
/// where GSP-RM's layout puts FRTS (1 MiB right below WPR2's 128 KiB aligned end, under the
/// VGA workspace, D522); the region the GPU then reports.
fn run_fwsec(
    probe: &mut dyn Probe,
    registers: PhysicalRange,
    fwsec: &nvidia_fwsec::Fwsec<'_>,
) -> Result<PhysicalRange, &'static str> {
    let length = usize::try_from(registers.len()).map_err(|_| "BAR 0 is too large")?;
    let base = probe
        .map_window(registers)
        .map_err(|_| "BAR 0 could not be mapped")?;
    // SAFETY: `map_window` mapped BAR 0, `length` bytes, as device memory for good; only this
    // probe drives the GPU's registers.
    let regs = unsafe { eggos_drivers::io::Mmio::new(base, length) };
    let wpr2 = |regs: &eggos_drivers::io::Mmio| {
        let low = u64::from(regs.read32(WPR2_LOW) >> 4) << 12;
        let high = u64::from(regs.read32(WPR2_HIGH) >> 4) << 12;
        (high > low).then(|| PhysicalRange {
            start: low,
            end: high + (1 << 12),
        })
    };
    if let Some(region) = wpr2(&regs) {
        return Ok(region);
    }
    let (video_memory, bios_start) = video_memory(&regs);
    if video_memory < 4 * FRTS_BYTES {
        return Err("the usable video memory is not known");
    }
    let offset = nvidia_gsp_boot::frts_offset(bios_start).ok_or("no room for FRTS")?;
    let version = nvidia_falcon::fuse_version(&regs, fwsec.ucode_id);
    let dmem = nvidia_fwsec::prepare_frts(fwsec, offset, version)
        .map_err(|_| "FWSEC has no interface or no signature for the fuse")?;
    let imem_bytes = fwsec.imem_bytes as usize;
    let image = super::dma::DmaRegion::allocate(imem_bytes + dmem.len())
        .map_err(|_| "no memory for FWSEC")?;
    // SAFETY: the region holds at least `imem_bytes + dmem.len()` bytes, mapped for the CPU
    // and owned by this probe until FWSEC has run.
    unsafe {
        let target = eggos_drivers::dma::DmaMemory::as_mut_ptr(&image);
        core::ptr::copy_nonoverlapping(fwsec.ucode.as_ptr(), target, imem_bytes);
        core::ptr::copy_nonoverlapping(dmem.as_ptr(), target.add(imem_bytes), dmem.len());
    }
    // SAFETY: as above, for the Falcon's own use of the same registers.
    let falcon_regs = unsafe { eggos_drivers::io::Mmio::new(base, length) };
    let falcon = nvidia_falcon::Falcon::new(falcon_regs, nvidia_falcon::GSP_BASE);
    let mut deadline = probe.deadline(FWSEC_MS);
    falcon
        .reset(&mut || deadline.keep_waiting())
        .map_err(nvidia_falcon::FalconError::description)?;
    let program = nvidia_falcon::SecureProgram {
        image: eggos_drivers::dma::DmaMemory::dma_addr(&image).get(),
        imem_physical: fwsec.imem_physical,
        imem_virtual: fwsec.imem_virtual,
        imem_bytes: fwsec.imem_bytes,
        dmem_physical: fwsec.dmem_physical,
        dmem_bytes: dmem.len() as u32,
        signature_place: fwsec.signature_place,
        engine_mask: fwsec.engine_mask,
        ucode_id: fwsec.ucode_id,
    };
    let mut deadline = probe.deadline(FWSEC_MS);
    falcon
        .run_secure(nvidia_falcon::GSP_FBIF, &program, (0, 0), &mut || {
            deadline.keep_waiting()
        })
        .map_err(nvidia_falcon::FalconError::description)?;
    if regs.read32(VBIOS_SCRATCH_FRTS) >> 16 != 0 {
        return Err("FWSEC reported an FRTS error");
    }
    wpr2(&regs).ok_or("FWSEC left no WPR2")
}

/// The video BIOS whole, through BAR 0's `PROM` window (D512): the expansion ROM BAR may show
/// only its first images, and FWSEC sits in the extended one.
fn read_prom(probe: &mut dyn Probe, registers: PhysicalRange) -> Option<Vec<u8>> {
    if registers.len() < PROM_START + PROM_BYTES {
        return None;
    }
    let window = PhysicalRange::from_start_length(registers.start + PROM_START, PROM_BYTES);
    let start = probe.map_window(window).ok()?;
    let mut rom = Vec::new();
    rom.try_reserve_exact(PROM_BYTES as usize).ok()?;
    for offset in 0..PROM_BYTES as usize / 4 {
        // SAFETY: `map_window` mapped the `PROM` window, `PROM_BYTES` long, as device memory for
        // good; reading it has no side effects.
        let word = unsafe { core::ptr::read_volatile((start + 4 * offset) as *const u32) };
        rom.extend_from_slice(&word.to_le_bytes());
    }
    Some(rom)
}

fn read_gsp_memories(probe: &mut dyn Probe, registers: PhysicalRange) -> Option<(usize, usize)> {
    let end = nvidia_falcon::GSP_BASE as u64 + GSP_BLOCK_BYTES;
    if registers.len() < end {
        return None;
    }
    let block = PhysicalRange::from_start_length(
        registers.start + nvidia_falcon::GSP_BASE as u64,
        GSP_BLOCK_BYTES,
    );
    let start = probe.map_window(block).ok()?;
    // SAFETY: `map_window` mapped GSP's Falcon block, `GSP_BLOCK_BYTES` long, as device memory
    // for good; reading its hardware configuration has no side effects.
    let regs = unsafe { eggos_drivers::io::Mmio::new(start, GSP_BLOCK_BYTES as usize) };
    let falcon = nvidia_falcon::Falcon::new(regs, 0);
    Some(falcon.memory_bytes())
}

fn read_discovery(probe: &mut dyn Probe, aperture: PhysicalRange) -> Option<Vec<u8>> {
    if aperture.len() < OFFSET_FROM_TOP {
        return None;
    }
    let region = PhysicalRange::from_start_length(aperture.end - OFFSET_FROM_TOP, OFFSET_FROM_TOP);
    let start = probe.map_window(region).ok()?;
    let mut binary = Vec::new();
    binary.try_reserve_exact(OFFSET_FROM_TOP as usize).ok()?;
    for offset in 0..OFFSET_FROM_TOP as usize {
        // SAFETY: `map_window` mapped the aperture's top `OFFSET_FROM_TOP` bytes as device
        // memory for good; video memory there is the firmware's discovery binary, which only
        // this probe reads.
        binary.push(unsafe { core::ptr::read_volatile((start + offset) as *const u8) });
    }
    Some(binary)
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
    if let Some(interpreter) = Interpreter::new(&image) {
        let tables = (0..ATOM_COMMAND_TABLES)
            .filter(|&table| interpreter.has_table(table))
            .count();
        let _ = write!(line, " {tables} command tables");
    }
    if let Some(config) = probe.pci_config() {
        eggos_drivers::pci::enable_memory_and_bus_master(config, function.address);
    }
    let display = read_discovery(probe, node.regions[0]).as_deref().and_then(display_block);
    match display {
        Some(display) => {
            let (major, minor, revision) = display.version;
            let firmware = dmcub_file_name(display.version);
            let _ = write!(line, " dcn {major}.{minor}.{revision} needs {firmware}");
        }
        None => line.push_str(" no IP discovery in the aperture"),
    }
    line.push('\n');
    SUMMARY.lock().push_str(&line);
    if let Some(display) = display
        && display.base_count > amd_dcn::SEGMENT
        && let Ok(segment_base) = u32::try_from(display.bases[amd_dcn::SEGMENT])
        && let Ok(clock_segment_base) = u32::try_from(display.bases[amd_dcn::CLOCK_SEGMENT])
    {
        let taken = super::amd_display::take(
            probe,
            display.version,
            (segment_base, clock_segment_base),
            node.regions[REGISTER_BAR],
            node.regions[0],
        );
        if let Ok(sinks) = taken {
            SUMMARY.lock().push_str(&sinks);
        }
    }
    Ok(())
}
