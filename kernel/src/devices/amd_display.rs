//! AMD's display engine on PCI, DCN 3.x (D496): the pipe the firmware lit, taken over at its
//! mode. The kernel draws into a framebuffer in ordinary cached memory; a flush copies the
//! damage into whichever of two surfaces in video memory the pipe is not showing (the
//! firmware's own and one right after it, written through a write-combining mapping of the
//! aperture), drains the writes and flips the pipe to it at the next vertical update, so a
//! frame is never shown half drawn. The pointer is the hub's 64x64 ARGB cursor, in video memory
//! after the second surface. A surface's GPU address is the firmware's plus its distance from
//! the firmware's framebuffer in the aperture, which maps video memory in order.
//!
//! DMCUB (D514): 2 MiB of video memory after the cursor, 1 MiB aligned, are kept for the
//! display microcontroller; once the file systems are mounted a thread waits for the
//! `display-firmware` component's DMCUB file for this DCN version, copies its instructions and
//! data there, and starts it ([`eggos_drivers::gpu::amd_dmcub`]).
//!
//! Modes (D521): when the lit pipe drives a DisplayPort stream on the DTO, the display can
//! change size: the new size's CVT reduced blanking timing at 60 Hz, up to the firmware mode's
//! pixel rate and the bytes of the firmware's framebuffer, shown blank from both surfaces.

use super::{Deadline, Probe, ProbeError};
use crate::display::{Mode, Rect, Scanout};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use eggos_drivers::gpu::amd_dcn::{self, DisplayEngine, Refusal};
use eggos_drivers::gpu::amd_discovery::{dmcub_file_name, dmcub_firmware};
use eggos_drivers::edid::Timing;
use eggos_drivers::gpu::amd_dmcub::{self, Dmcub};
use eggos_drivers::io::Mmio;
use eggos_hal::PhysicalRange;
use eggos_hal::handoff::pixel_format;

const MAX_PENDING: usize = 16;
const REFRESH_HZ: u32 = 60;

struct Surface {
    pixels: usize,
    address: u64,
}

struct AmdDisplay {
    engine: DisplayEngine<Mmio>,
    dmcub: Dmcub<Mmio>,
    dmcub_block: Surface,
    mode: Mode,
    stride: usize,
    frame: Vec<u32>,
    surfaces: [Surface; 2],
    surface_bytes: usize,
    missing: [Vec<Rect>; 2],
    front: Option<usize>,
    cursor: Surface,
    hotspot: (u32, u32),
    sinks: String,
}

// SAFETY: the registers and the surfaces belong to this scanout alone once installed, and the
// display's lock serializes every use.
unsafe impl Send for AmdDisplay {}

impl AmdDisplay {
    fn copy(&self, surface: usize, rect: Rect) {
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
            let offset = y as usize * self.stride + rect.x as usize * 4;
            // SAFETY: the surface maps `stride * height` bytes write-combining for good and the
            // clipped row lies inside it; only this scanout writes there.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    row.as_ptr(),
                    (self.surfaces[surface].pixels + offset) as *mut u8,
                    row.len(),
                );
            }
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
        let hidden = self.front.map_or(1, |front| 1 - front);
        let missing = core::mem::take(&mut self.missing[hidden]);
        for rect in missing.iter().chain(rects) {
            self.copy(hidden, *rect);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Self::remember(&mut self.missing[1 - hidden], rects);
        self.engine.flip(self.surfaces[hidden].address);
        self.front = Some(hidden);
        rects.len()
    }
}

impl Scanout for AmdDisplay {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        &mut self.frame
    }

    fn flush(&mut self, rects: &[Rect], deadline: &mut Deadline) -> Result<usize, &'static str> {
        while !self.engine.flip_done() {
            if !deadline.keep_waiting() {
                return Err(Refusal::FlipTimedOut.description());
            }
            core::hint::spin_loop();
        }
        Ok(self.post(rects))
    }

    fn sinks(&self) -> String {
        self.sinks.clone()
    }

    fn draws_pointer(&self) -> bool {
        true
    }

    fn load_firmware(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        let firmware = dmcub_firmware(bytes).ok_or("not a display microcontroller firmware")?;
        let (code_start, code_bytes) = firmware.instructions;
        let (data_start, data_bytes) = firmware.data;
        let layout = amd_dmcub::layout(code_bytes as u64, data_bytes as u64, BIOS_BYTES);
        if layout.total > DMCUB_BYTES {
            return Err("the display microcontroller's firmware is too large");
        }
        let block = self.dmcub_block.pixels as *mut u8;
        // SAFETY: the block maps `DMCUB_BYTES` of video memory write-combining for good, kept
        // for the microcontroller, which is stopped while this writes; the layout fits it.
        unsafe {
            core::ptr::write_bytes(block, 0, layout.total as usize);
            core::ptr::copy_nonoverlapping(
                bytes[code_start..].as_ptr(),
                block.add(layout.instructions.0 as usize),
                code_bytes,
            );
            core::ptr::copy_nonoverlapping(
                bytes[data_start..].as_ptr(),
                block.add(layout.data.0 as usize),
                data_bytes,
            );
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let mut left = DMCUB_POLLS;
        self.dmcub
            .start(self.dmcub_block.address, &layout, &mut || {
                left = left.saturating_sub(1);
                left > 0
            })
            .map_err(amd_dmcub::DmcubError::description)
    }

    fn pointer_image(&mut self) -> Option<&mut [u32]> {
        // SAFETY: the cursor maps `CURSOR_BYTES` of video memory write-combining for good,
        // aligned for its pixels; `&mut self` makes the borrow unique.
        Some(unsafe {
            core::slice::from_raw_parts_mut(
                self.cursor.pixels as *mut u32,
                amd_dcn::CURSOR_BYTES / 4,
            )
        })
    }

    fn show_pointer(
        &mut self,
        hotspot: (u32, u32),
        at: (u32, u32),
        deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        self.hotspot = hotspot;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.move_pointer(at, deadline)
    }

    fn move_pointer(
        &mut self,
        at: (u32, u32),
        _deadline: &mut Deadline,
    ) -> Result<(), &'static str> {
        let x = at.0 as i32 - self.hotspot.0 as i32;
        let y = at.1 as i32 - self.hotspot.1 as i32;
        self.engine.show_cursor(self.cursor.address, x, y);
        Ok(())
    }

    fn hide_pointer(&mut self, _deadline: &mut Deadline) -> Result<(), &'static str> {
        self.engine.hide_cursor();
        Ok(())
    }

    fn can_change_mode(&self) -> bool {
        self.engine.output().is_some()
    }

    fn change_mode(&mut self, mode: Mode, deadline: &mut Deadline) -> Result<(), &'static str> {
        let timing = Timing::reduced_blanking(mode.width, mode.height, REFRESH_HZ)
            .ok_or(Refusal::UnfitTiming.description())?;
        let pitch = mode.width.next_multiple_of(amd_dcn::PITCH_ALIGNMENT_PIXELS);
        let stride = pitch as usize * 4;
        let bytes = stride.checked_mul(mode.height as usize).ok_or("the mode is too large")?;
        if bytes > self.surface_bytes {
            return Err("the mode does not fit the display's surfaces");
        }
        let pixels = mode.width as usize * mode.height as usize;
        let mut frame = Vec::new();
        frame.try_reserve_exact(pixels).map_err(|_| "no memory for the display's frame")?;
        frame.resize(pixels, 0);
        for surface in &self.surfaces {
            // SAFETY: each surface maps `surface_bytes` of video memory write-combining for
            // good and `bytes` fits in it; only this scanout writes there.
            unsafe { core::ptr::write_bytes(surface.pixels as *mut u8, 0, bytes) };
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.engine
            .set_mode(&timing, self.surfaces[0].address, pitch, &mut || deadline.keep_waiting())
            .map_err(Refusal::description)?;
        self.mode = mode;
        self.stride = stride;
        self.frame = frame;
        self.front = Some(0);
        self.missing = [Vec::new(), vec![Rect::new(0, 0, mode.width, mode.height)]];
        Ok(())
    }
}

/// Where the second surface, the cursor and DMCUB's block go after the firmware's
/// framebuffer, `bytes` long: their offsets from its start, each aligned for its user.
pub fn placement(bytes: u64) -> (u64, u64, u64) {
    let second = bytes.next_multiple_of(amd_dcn::SURFACE_ALIGNMENT);
    let cursor = (second + bytes).next_multiple_of(amd_dcn::SURFACE_ALIGNMENT);
    let dmcub = (cursor + amd_dcn::CURSOR_BYTES as u64).next_multiple_of(DMCUB_ALIGNMENT);
    (second, cursor, dmcub)
}

const DMCUB_BYTES: u64 = 2 << 20;
const DMCUB_ALIGNMENT: u64 = 1 << 20;
const BIOS_BYTES: u64 = 128 << 10;
const DMCUB_POLLS: u32 = 50_000_000;
const FIRMWARE_FOLDER: &str = "/usr/lib/firmware/amdgpu";
const LARGEST_FIRMWARE: u64 = 1 << 20;
const FIRMWARE_RETRY: core::time::Duration = core::time::Duration::from_secs(10);

static FIRMWARE_VERSION: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn read_firmware(path: &str) -> Option<Vec<u8>> {
    let root = crate::vfs::root();
    let node = crate::vfs::resolve(root, root, path.as_bytes()).ok()?;
    let size = node.attributes().size;
    if node.kind() != crate::vfs::NodeKind::Regular || size > LARGEST_FIRMWARE {
        return None;
    }
    let bytes = crate::linux::exec::ProgramBytes::read(node, size, size).ok()?;
    Some(bytes.bytes().to_vec())
}

/// Starts the thread that loads DMCUB's firmware once the `display-firmware` component's file
/// for the taken engine's DCN version is there (D514); whether an AMD display was taken.
pub fn start_firmware<P: eggos_hal::Platform>() -> bool {
    let packed = FIRMWARE_VERSION.load(core::sync::atomic::Ordering::Acquire);
    if packed == 0 {
        return false;
    }
    let version = ((packed >> 16) as u8, (packed >> 8) as u8, packed as u8);
    let path = alloc::format!("{FIRMWARE_FOLDER}/{}", dmcub_file_name(version));
    crate::thread::spawn::<P, _>(move || {
        let bytes = loop {
            if let Some(bytes) = read_firmware(&path) {
                break bytes;
            }
            crate::thread::sleep::<P>(FIRMWARE_RETRY);
        };
        let _ = crate::display::load_firmware(&bytes);
    })
    .is_ok()
}

/// Takes the pipe the firmware lit on the AMD GPU whose display block (DCN `version`, segments 2
/// and 1 at `segment_bases` dwords) is behind the register BAR `registers`, when the firmware's
/// framebuffer lies in its video memory aperture `aperture` with room after it for a second
/// surface and the cursor; installs it as the display. Why not, otherwise.
pub fn take(
    probe: &mut dyn Probe,
    version: (u8, u8, u8),
    segment_bases: (u32, u32),
    registers: PhysicalRange,
    aperture: PhysicalRange,
) -> Result<String, ProbeError> {
    if !amd_dcn::knows(version) {
        return Err(ProbeError::Refused(Refusal::UnknownVersion.description()));
    }
    if crate::display::mode().is_some() {
        return Err(ProbeError::Refused("a display is already in use"));
    }
    let framebuffer = probe
        .boot_framebuffer()
        .ok_or(ProbeError::Refused("the firmware left no framebuffer"))?;
    if framebuffer.format != pixel_format::BGRX {
        return Err(ProbeError::Refused("the firmware's framebuffer is not in BGRX order"));
    }
    let stride = u64::from(framebuffer.stride_pixels) * 4;
    let bytes = stride * u64::from(framebuffer.height);
    let (second, cursor, dmcub) = placement(bytes);
    let end = dmcub + DMCUB_BYTES;
    if framebuffer.address < aperture.start || framebuffer.address + end > aperture.end {
        return Err(ProbeError::Refused("no room in the aperture after the firmware's framebuffer"));
    }
    let length = usize::try_from(registers.len())
        .map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let base = probe.map_window(registers)?;
    // SAFETY: `map_window` mapped the register BAR, `length` bytes, as device memory for good,
    // and only this driver uses the display block's registers.
    let regs = unsafe { Mmio::new(base, length) };
    // SAFETY: as above; the microcontroller's registers are the display block's too.
    let dmcub_regs = unsafe { Mmio::new(base, length) };
    let (segment_base, _) = segment_bases;
    let (engine, lit) = DisplayEngine::take(
        regs,
        version,
        segment_bases,
        (framebuffer.width, framebuffer.height),
    )
    .map_err(|refusal| ProbeError::Refused(refusal.description()))?;
    if u64::from(lit.pitch_pixels) != u64::from(framebuffer.stride_pixels) {
        return Err(ProbeError::Refused("the pipe's pitch is not the framebuffer's"));
    }
    let surfaces = PhysicalRange::from_start_length(framebuffer.address, end);
    let mapped = probe.map_write_combining(surfaces)?;
    let surface = |offset: u64| Surface {
        pixels: mapped + offset as usize,
        address: lit.address + offset,
    };
    let mode = Mode::new(framebuffer.width, framebuffer.height);
    let mut frame = Vec::new();
    frame
        .try_reserve_exact(mode.width as usize * mode.height as usize)
        .map_err(|_| ProbeError::Refused("no memory for the display's frame"))?;
    frame.resize(mode.width as usize * mode.height as usize, 0);
    let mut sinks = alloc::format!(
        "amd dcn {}.{}.{} pipe {} {}x{} at the firmware's mode\n",
        version.0,
        version.1,
        version.2,
        lit.pipe,
        mode.width,
        mode.height
    );
    if let Some(output) = engine.output() {
        sinks.push_str(&alloc::format!(
            "amd dp stream {} on timing generator {}, modes up to {} kHz\n",
            output.stream,
            output.timing_generator,
            output.link_pixel_hz / 1000
        ));
    }
    let scanout = AmdDisplay {
        engine,
        dmcub: Dmcub::new(dmcub_regs, segment_base),
        dmcub_block: surface(dmcub),
        mode,
        stride: stride as usize,
        frame,
        surfaces: [surface(0), surface(second)],
        surface_bytes: bytes as usize,
        missing: [Vec::new(), vec![Rect::new(0, 0, mode.width, mode.height)]],
        front: Some(0),
        cursor: surface(cursor),
        hotspot: (0, 0),
        sinks: sinks.clone(),
    };
    crate::display::install(scanout)
        .map_err(|_| ProbeError::Refused("a display is already in use"))?;
    let packed = u32::from(version.0) << 16 | u32::from(version.1) << 8 | u32::from(version.2);
    FIRMWARE_VERSION.store(packed, core::sync::atomic::Ordering::Release);
    Ok(sinks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_surface_and_the_cursor_follow_the_firmware_s_aligned() {
        assert_eq!(placement(1920 * 4 * 1080), (0x7f_0000, 0xfe_0000, 0x100_0000));
        assert_eq!(placement(0x10000), (0x10000, 0x20000, 0x10_0000));
    }
}
