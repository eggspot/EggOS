//! The firmware's framebuffer (D236): the one UEFI's graphics output left set up, which EggOS's
//! UEFI loader passes on. With no display device of its own the kernel draws straight into it,
//! mapped as device memory; the screen shows each pixel as it is written, so a flush only orders
//! the writes. Rows are the framebuffer's stride long: columns past the visible width are drawn
//! but not shown.

use super::{Deadline, Mode, Rect, Scanout};
use crate::memory::Memory;
use core::fmt::Write;
use eggos_hal::handoff::{Framebuffer, pixel_format};
use eggos_hal::{PhysicalRange, Platform};

/// What the console says before the mode of the firmware framebuffer the kernel took.
pub const LINE_PREFIX: &str = "boot: firmware framebuffer ";

/// The firmware's framebuffer, mapped.
pub struct BootFramebuffer {
    pixels: *mut u32,
    mode: Mode,
}

// SAFETY: the pixels are device memory the scanout owns alone once installed: nothing else maps
// or writes the framebuffer, and the display's lock serializes every use.
unsafe impl Send for BootFramebuffer {}

impl core::fmt::Debug for BootFramebuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BootFramebuffer")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl Scanout for BootFramebuffer {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        let count = self.mode.width as usize * self.mode.height as usize;
        // SAFETY: `pixels` maps `count` pixels of the firmware's framebuffer for as long as the
        // kernel runs, owned by this scanout, and `&mut self` makes the borrow unique.
        unsafe { core::slice::from_raw_parts_mut(self.pixels, count) }
    }

    fn flush(&mut self, rects: &[Rect], _deadline: &mut Deadline) -> Result<usize, &'static str> {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Ok(rects.len())
    }
}

/// The mode the framebuffer is drawn in: its stride by its height.
pub fn mode_of(framebuffer: &Framebuffer) -> Mode {
    Mode::new(framebuffer.stride_pixels, framebuffer.height)
}

/// Why the firmware's framebuffer was not taken.
pub fn refusal(framebuffer: &Framebuffer) -> Option<&'static str> {
    if framebuffer.format != pixel_format::BGRX {
        return Some("its pixels are in RGB order, which the kernel does not draw yet");
    }
    let bytes = mode_of(framebuffer).bytes();
    if bytes == usize::MAX || bytes as u64 > framebuffer.size {
        return Some("its rows run past its memory");
    }
    None
}

/// Takes the firmware's framebuffer as the display when there is one and no display device was
/// found: its mode, or why not; `None` when the firmware left none.
pub fn install<P: Platform>(memory: &mut Memory<P>) -> Option<Result<Mode, &'static str>> {
    let framebuffer = P::boot_framebuffer()?;
    if let Some(reason) = refusal(&framebuffer) {
        return Some(Err(reason));
    }
    let mode = mode_of(&framebuffer);
    let region = PhysicalRange::from_start_length(framebuffer.address, mode.bytes() as u64);
    let Ok(virt) = crate::devices::window::map::<P>(memory, region) else {
        return Some(Err("it could not be mapped"));
    };
    let scanout = BootFramebuffer {
        pixels: virt as *mut u32,
        mode,
    };
    Some(
        super::install(scanout)
            .map(|()| mode)
            .map_err(|_| "a display is already in use"),
    )
}

/// Writes what [`install`] did.
pub fn report<W: Write>(
    console: &mut W,
    installed: Option<Result<Mode, &'static str>>,
) -> core::fmt::Result {
    match installed {
        None => Ok(()),
        Some(Ok(mode)) => writeln!(console, "{LINE_PREFIX}{}x{}", mode.width, mode.height),
        Some(Err(reason)) => writeln!(console, "{LINE_PREFIX}not used: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framebuffer() -> Framebuffer {
        Framebuffer {
            address: 0x8000_0000,
            size: 1376 * 768 * 4,
            width: 1366,
            height: 768,
            stride_pixels: 1376,
            format: pixel_format::BGRX,
        }
    }

    #[test]
    fn the_framebuffer_is_drawn_at_its_stride() {
        assert_eq!(mode_of(&framebuffer()), Mode::new(1376, 768));
        assert_eq!(refusal(&framebuffer()), None);
    }

    #[test]
    fn rgb_order_and_short_memory_are_refused() {
        let rgb = Framebuffer {
            format: pixel_format::RGBX,
            ..framebuffer()
        };
        assert!(refusal(&rgb).is_some());
        let short = Framebuffer {
            size: 1000,
            ..framebuffer()
        };
        assert!(refusal(&short).is_some());
    }
}
