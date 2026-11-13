//! Intel's integrated display engine on PCI (D268): the plane the firmware lit, taken over at its
//! mode. The kernel draws into a framebuffer in ordinary cached memory; a flush copies the damage
//! into whichever of two buffers the plane is not showing, writes those lines back from the
//! caches (the display engine does not snoop them) and flips the plane to it, so a frame is never
//! shown half drawn. The pointer is the engine's 64x64 ARGB cursor plane.

use super::dma::DmaRegion;
use super::interrupts::DeviceEvents;
use super::{
    Deadline, DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError,
    ProbePass,
};
use crate::display::{Mode, Rect, Scanout};
use alloc::vec;
use alloc::vec::Vec;
use eggos_drivers::dma::DmaMemory;
use eggos_drivers::gpu::intel::{self, DisplayEngine, LitPlane, Refusal};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;

const DISPLAY_CONTROLLER: u8 = 0x03;
const VGA_COMPATIBLE: u8 = 0x00;
const CHUNK_BYTES: usize = 4 << 20;
const MAX_PENDING: usize = 16;

/// Intel's integrated graphics (PCI class 03h, subclass 00h, vendor 8086h).
pub static INTEL_DISPLAY_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "intel-display",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: DISPLAY_CONTROLLER,
        subclass: VGA_COMPATIBLE,
    }],
    probe: probe_intel_display,
};

struct Buffer {
    chunks: Vec<DmaRegion>,
    surface: u32,
}

impl Buffer {
    fn allocate(bytes: usize) -> Result<Self, ProbeError> {
        let mut chunks = Vec::new();
        let mut left = bytes;
        while left > 0 {
            let size = left.min(CHUNK_BYTES);
            let chunk = DmaRegion::allocate(size)
                .map_err(|_| ProbeError::Refused("no memory for the display's buffers"))?;
            left -= size;
            chunks.push(chunk);
        }
        Ok(Self { chunks, surface: 0 })
    }

    fn pages(&self, bytes: usize) -> impl Iterator<Item = u64> + '_ {
        let pages = bytes.div_ceil(intel::GTT_PAGE as usize);
        self.chunks
            .iter()
            .flat_map(|chunk| {
                let base = chunk.dma_addr().get();
                (0..chunk.len() as u64 / intel::GTT_PAGE)
                    .map(move |page| base + page * intel::GTT_PAGE)
            })
            .take(pages)
    }

    fn write(&self, mut offset: usize, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let chunk = &self.chunks[offset / CHUNK_BYTES];
            let within = offset % CHUNK_BYTES;
            let length = bytes.len().min(CHUNK_BYTES - within);
            // SAFETY: the chunk is this buffer's own DMA memory, at least `within + length` bytes
            // long since every chunk but the last is `CHUNK_BYTES` and offsets stay inside the
            // frame; the display engine only reads it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    chunk.as_mut_ptr().add(within),
                    length,
                );
                eggos_hal::cache::write_back_for_device(chunk.as_mut_ptr().add(within), length);
            }
            offset += length;
            bytes = &bytes[length..];
        }
    }
}

fn fence() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

struct IntelDisplay {
    engine: DisplayEngine<Mmio>,
    mode: Mode,
    stride: u32,
    frame: Vec<u32>,
    buffers: [Buffer; 2],
    missing: [Vec<Rect>; 2],
    front: Option<usize>,
    cursor: DmaRegion,
    cursor_surface: u32,
    hotspot: (u32, u32),
    events: Option<&'static DeviceEvents>,
    posted_rects: usize,
    sinks: alloc::string::String,
}

const DIGITAL_PORTS: usize = 4;
const SINK_ANSWER_MS: u64 = 50;
const MODE_SET_MS: u64 = 500;
const BITS_PER_PIXEL: u32 = 24;

static FOUND_DEVICE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
const FIRMWARE_FOLDER: &str = "/usr/lib/firmware/i915";
const LARGEST_FIRMWARE: u64 = 1 << 20;
const FIRMWARE_RETRY: core::time::Duration = core::time::Duration::from_secs(10);

/// Starts the thread that loads the display microcontroller's firmware once the
/// `display-firmware` component's file is there (D444); whether a display needs one.
pub fn start_firmware<P: eggos_hal::Platform>() -> bool {
    let device = FOUND_DEVICE.load(core::sync::atomic::Ordering::Acquire) as u16;
    let Some(file) = eggos_drivers::gpu::intel_dmc::file_for(device) else {
        return false;
    };
    let path = alloc::format!("{FIRMWARE_FOLDER}/{file}");
    crate::thread::spawn::<P, _>(move || {
        let bytes = loop {
            if let Some(bytes) = super::read_firmware(&path, LARGEST_FIRMWARE) {
                break bytes;
            }
            crate::thread::sleep::<P>(FIRMWARE_RETRY);
        };
        let _ = crate::display::load_firmware(&bytes);
    })
    .is_ok()
}

static VBLANK_EVENTS: DeviceEvents = DeviceEvents::new();

// SAFETY: the registers and buffers belong to this scanout alone once installed, and the
// display's lock serializes every use.
unsafe impl Send for IntelDisplay {}

impl IntelDisplay {
    fn copy(&self, buffer: usize, rect: Rect) {
        let rect = rect.clipped(self.mode.width, self.mode.height);
        let mut row = vec![0u8; rect.width as usize * 4];
        for y in rect.y..rect.y + rect.height {
            let start = (y * self.mode.width + rect.x) as usize;
            for (bytes, pixel) in row
                .chunks_exact_mut(4)
                .zip(&self.frame[start..start + rect.width as usize])
            {
                bytes.copy_from_slice(&pixel.to_le_bytes());
            }
            let offset = y as usize * self.stride as usize + rect.x as usize * 4;
            self.buffers[buffer].write(offset, &row);
        }
    }

    fn remember(missing: &mut Vec<Rect>, rects: &[Rect]) {
        missing.extend_from_slice(rects);
        if missing.len() > MAX_PENDING
            && let Some(first) = missing.first().copied()
        {
            let all = missing.iter().fold(first, |all, rect| all.union(*rect));
            missing.clear();
            missing.push(all);
        }
    }

    fn post(&mut self, rects: &[Rect]) -> usize {
        let hidden = self.front.map_or(0, |front| 1 - front);
        let missing = core::mem::take(&mut self.missing[hidden]);
        for rect in missing.iter().chain(rects) {
            self.copy(hidden, *rect);
        }
        fence();
        Self::remember(&mut self.missing[1 - hidden], rects);
        self.engine.flip(self.buffers[hidden].surface, self.stride);
        self.front = Some(hidden);
        rects.len()
    }

    fn cursor_corner(&self, at: (u32, u32)) -> (i32, i32) {
        (
            at.0 as i32 - self.hotspot.0 as i32,
            at.1 as i32 - self.hotspot.1 as i32,
        )
    }
}

impl Scanout for IntelDisplay {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        &mut self.frame
    }

    fn flush(&mut self, rects: &[Rect], deadline: &mut Deadline) -> Result<usize, &'static str> {
        if let Some(front) = self.front {
            while !self.engine.showing(self.buffers[front].surface) {
                if !deadline.keep_waiting() {
                    return Err(Refusal::FlipTimedOut.description());
                }
                core::hint::spin_loop();
            }
        }
        Ok(self.post(rects))
    }

    fn flush_events(&self) -> Option<&'static DeviceEvents> {
        self.events
    }

    fn start_flush(&mut self, rects: &[Rect]) -> Result<usize, &'static str> {
        if let Some(front) = self.front
            && !self.engine.showing(self.buffers[front].surface)
        {
            return Err(Refusal::FlipTimedOut.description());
        }
        self.posted_rects = self.post(rects);
        Ok(self.posted_rects)
    }

    fn finish_flush(&mut self) -> Result<Option<usize>, &'static str> {
        self.engine.acknowledge_vblank();
        Ok(match self.front {
            Some(front) if !self.engine.showing(self.buffers[front].surface) => None,
            _ => Some(self.posted_rects),
        })
    }

    fn abandon_flush(&mut self) {}

    fn sinks(&self) -> alloc::string::String {
        self.sinks.clone()
    }

    fn load_firmware(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        let dmc = eggos_drivers::gpu::intel_dmc::parse(bytes, 0, 0)
            .ok_or("not display firmware for this device")?;
        self.engine.load_dmc(&dmc);
        Ok(())
    }

    fn draws_pointer(&self) -> bool {
        true
    }

    fn pointer_image(&mut self) -> Option<&mut [u32]> {
        // SAFETY: the cursor region is this scanout's own, `CURSOR_BYTES` long and aligned to a
        // frame; `&mut self` makes the borrow unique.
        Some(unsafe {
            core::slice::from_raw_parts_mut(
                self.cursor.as_mut_ptr().cast::<u32>(),
                intel::CURSOR_BYTES / 4,
            )
        })
    }

    fn show_pointer(
        &mut self,
        hotspot: (u32, u32),
        at: (u32, u32),
        _deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        self.hotspot = hotspot;
        // SAFETY: the cursor region is mapped for `CURSOR_BYTES`.
        unsafe {
            eggos_hal::cache::write_back_for_device(self.cursor.as_mut_ptr(), intel::CURSOR_BYTES)
        };
        fence();
        let (x, y) = self.cursor_corner(at);
        self.engine.show_cursor(self.cursor_surface, x, y);
        Ok(())
    }

    fn move_pointer(
        &mut self,
        at: (u32, u32),
        _deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        let (x, y) = self.cursor_corner(at);
        self.engine.show_cursor(self.cursor_surface, x, y);
        Ok(())
    }

    fn hide_pointer(&mut self, _deadline: &mut Deadline) -> Result<(), &'static str> {
        self.engine.hide_cursor(self.cursor_surface);
        Ok(())
    }
}

/// Where the driver puts its surfaces in a global GTT translating `span` bytes, below its top:
/// the cursor, then the two buffers of `buffer_bytes` each, all aligned for the planes; `None`
/// when they would reach the firmware's surface, which ends at `firmware_end`.
pub fn surfaces(span: u64, buffer_bytes: u64, firmware_end: u64) -> Option<(u32, [u32; 2])> {
    let align_down = |at: u64| at & !(intel::SURFACE_ALIGNMENT - 1);
    let cursor = span.checked_sub(intel::SURFACE_ALIGNMENT)?;
    let second = align_down(cursor.checked_sub(buffer_bytes)?);
    let first = align_down(second.checked_sub(buffer_bytes)?);
    if first < firmware_end || span > 1 << 32 {
        return None;
    }
    Some((cursor as u32, [first as u32, second as u32]))
}

fn probe_intel_display(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if function.vendor_id != intel::VENDOR_INTEL || !intel::is_supported(function.device_id) {
        return Err(ProbeError::Refused(Refusal::UnknownDevice.description()));
    }
    if crate::display::mode().is_some() {
        return Err(ProbeError::Refused("a display is already in use"));
    }
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    let region = node.regions[0];
    if region.is_empty() {
        return Err(ProbeError::Refused(
            "the engine's registers are not in memory space",
        ));
    }
    let length = usize::try_from(region.len())
        .map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let graphics_control = config.read32(function.address, intel::GRAPHICS_CONTROL) as u16;
    pci::enable_memory_and_bus_master(config, function.address);
    let base = probe.map_window(region)?;
    // SAFETY: BAR 0 of Intel's integrated graphics is its registers and, in its second half, its
    // global GTT, mapped as device memory by `map_window` for as long as the kernel runs; only
    // this driver drives the display engine.
    let regs = unsafe { Mmio::new(base, length) };
    let (engine, plane) = DisplayEngine::take(regs, length, function.device_id)
        .map_err(|refusal| ProbeError::Refused(refusal.description()))?;
    let LitPlane {
        width,
        height,
        stride_bytes,
        surface,
        ..
    } = plane;
    let stride = intel::stride_for(width);
    let buffer_bytes = (stride as u64 * u64::from(height)).next_multiple_of(intel::GTT_PAGE);
    let firmware_end = u64::from(surface) + u64::from(stride_bytes) * u64::from(height);
    let (cursor_surface, [first, second]) = surfaces(
        intel::gtt_span(graphics_control),
        buffer_bytes,
        firmware_end,
    )
    .ok_or(ProbeError::Refused(Refusal::NoGttSpace.description()))?;
    let mut buffers = [
        Buffer::allocate(buffer_bytes as usize)?,
        Buffer::allocate(buffer_bytes as usize)?,
    ];
    for (buffer, at) in buffers.iter_mut().zip([first, second]) {
        buffer.surface = at;
        engine.map(u64::from(at), &mut buffer.pages(buffer_bytes as usize));
    }
    let cursor = DmaRegion::allocate(intel::CURSOR_BYTES)
        .map_err(|_| ProbeError::Refused("no memory for the cursor"))?;
    engine.map(
        u64::from(cursor_surface),
        &mut core::iter::once(cursor.dma_addr().get()).chain(
            (1..intel::CURSOR_BYTES as u64 / intel::GTT_PAGE)
                .map(|page| cursor.dma_addr().get() + page * intel::GTT_PAGE),
        ),
    );
    let mode = Mode::new(width, height);
    let whole = mode.rect();
    let scanout = IntelDisplay {
        engine,
        mode,
        stride,
        frame: vec![0; width as usize * height as usize],
        buffers,
        missing: [vec![whole], vec![whole]],
        front: None,
        cursor,
        cursor_surface,
        hotspot: (0, 0),
        events: None,
        posted_rects: 0,
        sinks: alloc::string::String::new(),
    };
    FOUND_DEVICE.store(
        u32::from(function.device_id),
        core::sync::atomic::Ordering::Release,
    );
    let mut scanout = scanout;
    let lit_ports = scanout.engine.lit_ports();
    let mut second_screen = None;
    let mut hdmi_screen = None;
    let mut mirrored = false;
    for port in 0..DIGITAL_PORTS {
        if scanout.engine.is_type_c(port) {
            continue;
        }
        let mut deadline = probe.deadline(SINK_ANSWER_MS);
        let (link, edid) = scanout
            .engine
            .probe_sink(port, &mut || deadline.keep_waiting());
        let (edid, hdmi) = match (link, edid) {
            (None, None) => {
                let mut deadline = probe.deadline(SINK_ANSWER_MS);
                match scanout
                    .engine
                    .probe_hdmi_sink(port, &mut || deadline.keep_waiting())
                {
                    Some(edid) => (Some(edid), true),
                    None => continue,
                }
            }
            (_, edid) => (edid, false),
        };
        if hdmi
            && lit_ports & 1 << port == 0
            && hdmi_screen.is_none()
            && let Some(timing) = edid.and_then(|edid| edid.preferred)
        {
            hdmi_screen = Some((port, timing));
        }
        if let (Some(link), Some(timing)) = (link, edid.and_then(|edid| edid.preferred))
            && lit_ports & 1 << port == 0
            && second_screen.is_none()
        {
            second_screen = Some((port, link, timing));
        }
        let port_name = (b'A' + port as u8) as char;
        let kind = if hdmi { " hdmi" } else { "" };
        let _ =
            core::fmt::Write::write_fmt(&mut scanout.sinks, format_args!("port {port_name}{kind}"));
        if let Some(edid) = edid {
            let maker = core::str::from_utf8(&edid.maker).unwrap_or("?");
            let _ = core::fmt::Write::write_fmt(
                &mut scanout.sinks,
                format_args!(" {maker} {}", edid.name()),
            );
            if let Some(mode) = edid.preferred {
                let _ = core::fmt::Write::write_fmt(
                    &mut scanout.sinks,
                    format_args!(
                        " preferred {}x{}@{}",
                        mode.width,
                        mode.height,
                        mode.refresh_mhz() / 1000
                    ),
                );
            }
        }
        if let Some(link) = link {
            let _ = core::fmt::Write::write_fmt(
                &mut scanout.sinks,
                format_args!(
                    " dp {}.{} x{}",
                    link.rate_centigbps / 100,
                    link.rate_centigbps % 100,
                    link.lanes
                ),
            );
        }
        scanout.sinks.push('\n');
    }
    for index in 0..scanout.engine.type_c_ports() {
        let mut deadline = probe.deadline(SINK_ANSWER_MS);
        let connected = scanout
            .engine
            .connect_type_c(index, &mut || deadline.keep_waiting());
        let Ok(connected) = connected else {
            continue;
        };
        let port_name = (b'A' + connected.port as u8) as char;
        let _ = core::fmt::Write::write_fmt(
            &mut scanout.sinks,
            format_args!("port {port_name} type-c"),
        );
        match connected.mode {
            eggos_drivers::gpu::intel_type_c::Mode::Thunderbolt => {
                scanout.sinks.push_str(" thunderbolt\n");
                continue;
            }
            eggos_drivers::gpu::intel_type_c::Mode::DisplayPortAlternate => {
                let pin = match connected.pin_assignment {
                    0 => '-',
                    pin => (b'A' + pin - 1) as char,
                };
                let _ = core::fmt::Write::write_fmt(
                    &mut scanout.sinks,
                    format_args!(" dp-alt x{} pin {pin}", connected.lanes),
                );
            }
        }
        let mut deadline = probe.deadline(SINK_ANSWER_MS);
        let (link, edid) = scanout
            .engine
            .probe_sink(connected.port, &mut || deadline.keep_waiting());
        if let Some(edid) = edid {
            let maker = core::str::from_utf8(&edid.maker).unwrap_or("?");
            let _ = core::fmt::Write::write_fmt(
                &mut scanout.sinks,
                format_args!(" {maker} {}", edid.name()),
            );
        }
        if let Some(link) = link {
            let _ = core::fmt::Write::write_fmt(
                &mut scanout.sinks,
                format_args!(
                    " dp {}.{} x{}",
                    link.rate_centigbps / 100,
                    link.rate_centigbps % 100,
                    link.lanes
                ),
            );
        }
        scanout.sinks.push('\n');
        if let (Some(mut link), Some(timing)) = (link, edid.and_then(|edid| edid.preferred))
            && second_screen.is_none()
        {
            link.lanes = link.lanes.min(connected.lanes);
            second_screen = Some((connected.port, link, timing));
        }
    }
    if let Some((port, link, timing)) = second_screen
        && intel::sets_modes(function.device_id)
        && let Some(pipe) = scanout.engine.free_pipe()
        && eggos_drivers::gpu::intel_modeset::fits_link(
            timing.clock_khz,
            BITS_PER_PIXEL,
            link.rate_centigbps,
            u32::from(link.lanes),
        )
    {
        let mut deadline = probe.deadline(MODE_SET_MS);
        let lit = scanout.engine.set_mode(
            pipe,
            port,
            &timing,
            usize::from(link.lanes),
            link.rate_code(),
            scanout.buffers[0].surface,
            stride,
            &mut || deadline.keep_waiting(),
        );
        if lit.is_ok() {
            mirrored = true;
            scanout.engine.mirror(pipe, (width, height), &timing);
            let port_name = (b'A' + port as u8) as char;
            let _ = core::fmt::Write::write_fmt(
                &mut scanout.sinks,
                format_args!("port {port_name} mirrors the first screen on pipe {pipe}\n"),
            );
        }
    }
    if let Some((port, timing)) = hdmi_screen
        && !mirrored
        && intel::is_display_9(function.device_id)
        && let Some(pipe) = scanout.engine.free_pipe()
    {
        let mut deadline = probe.deadline(MODE_SET_MS);
        let lit = scanout.engine.set_mode_hdmi(
            pipe,
            port,
            &timing,
            scanout.buffers[0].surface,
            stride,
            &mut || deadline.keep_waiting(),
        );
        if lit.is_ok() {
            scanout.engine.mirror(pipe, (width, height), &timing);
            let port_name = (b'A' + port as u8) as char;
            let _ = core::fmt::Write::write_fmt(
                &mut scanout.sinks,
                format_args!("port {port_name} (hdmi) mirrors the first screen on pipe {pipe}\n"),
            );
        }
    }
    if super::message_interrupts::connect(probe, id, &VBLANK_EVENTS).is_ok() {
        scanout.engine.interrupt_on_vblank(function.device_id);
        scanout.events = Some(&VBLANK_EVENTS);
    }
    crate::display::install(scanout).map_err(|_| ProbeError::Refused("a display is already in use"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surfaces_sit_below_the_gtt_s_top_and_clear_of_the_firmware_s() {
        let span = 2u64 << 30;
        let buffer = 1920 * 4 * 1080u64;
        let (cursor, [first, second]) = surfaces(span, buffer, 0x80_0000).unwrap();
        assert_eq!(u64::from(cursor), span - intel::SURFACE_ALIGNMENT);
        assert!(u64::from(second) + buffer <= u64::from(cursor));
        assert!(u64::from(first) + buffer <= u64::from(second));
        assert_eq!(u64::from(first) % intel::SURFACE_ALIGNMENT, 0);
        assert_eq!(surfaces(32 << 20, buffer, 16 << 20), None);
        assert_eq!(surfaces(0, buffer, 0), None);
    }
}
