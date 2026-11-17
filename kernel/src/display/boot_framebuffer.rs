//! The firmware's framebuffer (D236): the one UEFI's graphics output left set up, which EggOS's
//! UEFI loader passes on. With no display device of its own the kernel draws straight into it,
//! mapped write-combining (D245); the screen shows each pixel once its writes drain, so a flush
//! only orders and drains them. Rows are the framebuffer's stride long: columns past the visible
//! width are drawn but not shown. A framebuffer in another pixel order, or of 16-bit `r5g6b5`
//! pixels (D620), is drawn in a 32-bit copy in memory, converted into the framebuffer for the
//! rectangles each flush names.

use super::{Deadline, Mode, Rect, Scanout};
use crate::memory::Memory;
use alloc::vec::Vec;
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

/// A framebuffer whose pixels are not the kernel's `0x00RRGGBB` (D620): drawn in `copy`, one
/// `u32` a pixel at its visible width, and converted into the framebuffer at each flush.
pub struct ConvertedFramebuffer {
    device: *mut u8,
    stride_bytes: usize,
    format: u32,
    copy: Vec<u32>,
    mode: Mode,
}

// SAFETY: as for `BootFramebuffer`: the framebuffer's memory is this scanout's alone, and the
// display's lock serializes every use.
unsafe impl Send for ConvertedFramebuffer {}

impl core::fmt::Debug for ConvertedFramebuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConvertedFramebuffer")
            .field("mode", &self.mode)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// `pixel`, `0x00RRGGBB`, as 16-bit `r5g6b5`.
pub const fn rgb565(pixel: u32) -> u16 {
    let red = (pixel >> 19) & 0x1f;
    let green = (pixel >> 10) & 0x3f;
    let blue = (pixel >> 3) & 0x1f;
    ((red << 11) | (green << 5) | blue) as u16
}

/// `pixel`, `0x00RRGGBB`, with red and blue swapped: bytes red, green, blue in memory.
pub const fn rgbx(pixel: u32) -> u32 {
    ((pixel & 0xff) << 16) | (pixel & 0xff00) | ((pixel >> 16) & 0xff)
}

impl Scanout for ConvertedFramebuffer {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn pixels(&mut self) -> &mut [u32] {
        &mut self.copy
    }

    fn flush(&mut self, rects: &[Rect], _deadline: &mut Deadline) -> Result<usize, &'static str> {
        let width = self.mode.width as usize;
        let height = self.mode.height as usize;
        for rect in rects {
            let left = (rect.x as usize).min(width);
            let right = (rect.right() as usize).min(width);
            let top = (rect.y as usize).min(height);
            let bottom = (rect.bottom() as usize).min(height);
            for row in top..bottom {
                let source = &self.copy[row * width + left..row * width + right];
                // SAFETY: row `row` of the mapped framebuffer is `stride_bytes` long and lies
                // inside the mapping (`install` checked the rows against its size); columns
                // `left..right` are within the visible width, so within the row.
                let line = unsafe { self.device.add(row * self.stride_bytes) };
                for (column, &pixel) in (left..right).zip(source) {
                    match self.format {
                        pixel_format::RGB565 => {
                            // SAFETY: as above; 16-bit pixel `column` of the row.
                            unsafe {
                                line.cast::<u16>()
                                    .add(column)
                                    .write_volatile(rgb565(pixel))
                            }
                        }
                        _ => {
                            // SAFETY: as above; 32-bit pixel `column` of the row.
                            unsafe { line.cast::<u32>().add(column).write_volatile(rgbx(pixel)) }
                        }
                    }
                }
            }
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Ok(rects.len())
    }
}

/// The mode the framebuffer is drawn in: its stride by its height for one in the kernel's own
/// pixel order, else its visible width by its height (D620).
pub fn mode_of(framebuffer: &Framebuffer) -> Mode {
    match framebuffer.format {
        pixel_format::BGRX => Mode::new(framebuffer.stride_pixels, framebuffer.height),
        _ => Mode::new(framebuffer.width, framebuffer.height),
    }
}

fn mapped_bytes(framebuffer: &Framebuffer) -> u64 {
    u64::from(framebuffer.stride_pixels)
        .saturating_mul(u64::from(framebuffer.height))
        .saturating_mul(u64::from(pixel_format::bytes_per_pixel(framebuffer.format)))
}

/// Why the firmware's framebuffer was not taken.
pub fn refusal(framebuffer: &Framebuffer) -> Option<&'static str> {
    if !matches!(
        framebuffer.format,
        pixel_format::BGRX | pixel_format::RGBX | pixel_format::RGB565
    ) {
        return Some("its pixels are in a format the kernel does not draw");
    }
    if mode_of(framebuffer).bytes() == usize::MAX || mapped_bytes(framebuffer) > framebuffer.size {
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
    let region = PhysicalRange::from_start_length(framebuffer.address, mapped_bytes(&framebuffer));
    let Ok(virt) = crate::devices::window::map_write_combining::<P>(memory, region) else {
        return Some(Err("it could not be mapped"));
    };
    let installed = match framebuffer.format {
        pixel_format::BGRX => super::install(BootFramebuffer {
            pixels: virt as *mut u32,
            mode,
        })
        .map_err(|_| ()),
        format => {
            let mut copy = Vec::new();
            let pixels = mode.width as usize * mode.height as usize;
            if copy.try_reserve_exact(pixels).is_err() {
                return Some(Err("no memory for its copy"));
            }
            copy.resize(pixels, 0);
            super::install(ConvertedFramebuffer {
                device: virt as *mut u8,
                stride_bytes: framebuffer.stride_pixels as usize
                    * pixel_format::bytes_per_pixel(format) as usize,
                format,
                copy,
                mode,
            })
            .map_err(|_| ())
        }
    };
    Some(
        installed
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
    fn the_other_formats_are_drawn_at_their_width_and_short_memory_is_refused() {
        let rgb = Framebuffer {
            format: pixel_format::RGBX,
            ..framebuffer()
        };
        assert_eq!(refusal(&rgb), None);
        assert_eq!(mode_of(&rgb), Mode::new(1366, 768));
        let sixteen = Framebuffer {
            format: pixel_format::RGB565,
            size: 1376 * 768 * 2,
            ..framebuffer()
        };
        assert_eq!(refusal(&sixteen), None);
        assert_eq!(rgb565(0x00ff_0000), 0xf800);
        assert_eq!(rgb565(0x0000_ff00), 0x07e0);
        assert_eq!(rgbx(0x0012_3456), 0x0056_3412);
        let short = Framebuffer {
            size: 1000,
            ..framebuffer()
        };
        assert!(refusal(&short).is_some());
    }
}
