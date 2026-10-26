//! The display (D112): the framebuffer a display driver set up, which the kernel draws into and
//! shows. The virtio GPU's driver installs it today ([`crate::devices::virtio`], feature
//! `virtio-gpu`); the graphical terminal and the desktop (v0.3.0 items 13 and 14) draw through
//! it.
//!
//! [`draw`] lends the [`Framebuffer`] under the display's lock; what it changes is recorded as
//! damage ([`Framebuffer::damage`], [`Framebuffer::fill`]), and [`flush`] shows the damage and
//! nothing else. Nothing reaches the device while nothing is damaged.

use crate::devices::Deadline;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
pub use eggos_drivers::display::{Damage, Mode, Rect};
use eggos_hal::Platform;

/// The first frame's colour, as every pixel is: `0x00RRGGBB`. The design's `wallpaper.top`,
/// the boot screen's background (docs/ux/desktop-v0.3.md, sections 1 and 2).
pub const FIRST_FRAME_COLOUR: u32 = 0x000e_1726;

/// The [`pixel_hash`] of the first frame at 1280x800, the mode xtask asks QEMU for.
pub const FIRST_FRAME_HASH_1280X800: u64 = 0x1125_09e9_e81d_2325;

/// How long a flush polls the device before giving up.
pub const FLUSH_TIMEOUT_MS: u64 = 1000;

/// What a display driver lends the kernel: a framebuffer shown on a display output.
pub trait Scanout: Send {
    /// The framebuffer's mode.
    fn mode(&self) -> Mode;
    /// The framebuffer's pixels: `mode().width` per row, row after row, each `0x00RRGGBB`.
    fn pixels(&mut self) -> &mut [u32];
    /// Shows the pixels inside `rects` on the display, polling the device until `deadline`, and
    /// returns how many rectangles were shown.
    fn flush(&mut self, rects: &[Rect], deadline: &mut Deadline) -> Result<usize, &'static str>;
}

/// A display: a driver's [`Scanout`] and the damage drawn on it since its last flush.
pub struct Display {
    scanout: Box<dyn Scanout>,
    damage: Damage,
    flushes: u64,
}

impl Display {
    /// The display `scanout` shows, nothing damaged yet.
    pub fn new(scanout: Box<dyn Scanout>) -> Self {
        Self {
            scanout,
            damage: Damage::new(),
            flushes: 0,
        }
    }

    /// The framebuffer's mode.
    pub fn mode(&self) -> Mode {
        self.scanout.mode()
    }

    /// How many flushes have shown something.
    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Lends `draw` the framebuffer and returns what it returns.
    pub fn draw<R>(&mut self, draw: impl FnOnce(&mut Framebuffer<'_>) -> R) -> R {
        let mode = self.scanout.mode();
        let mut framebuffer = Framebuffer {
            pixels: self.scanout.pixels(),
            mode,
            damage: &mut self.damage,
        };
        draw(&mut framebuffer)
    }

    /// Shows what was drawn since the last flush, polling the device until `deadline`, and
    /// returns how many rectangles that took: 0 when nothing was drawn. When the device fails,
    /// the damage stays for the next flush.
    pub fn flush(&mut self, deadline: &mut Deadline) -> Result<usize, &'static str> {
        if self.damage.is_empty() {
            return Ok(0);
        }
        let shown = self.scanout.flush(self.damage.rects(), deadline)?;
        self.damage.clear();
        self.flushes += 1;
        Ok(shown)
    }
}

impl core::fmt::Debug for Display {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Display")
            .field("mode", &self.mode())
            .field("damage", &self.damage)
            .field("flushes", &self.flushes)
            .finish()
    }
}

static DISPLAY: LeafSpinLock<Option<Box<Display>>> = LeafSpinLock::new_leaf(None);

/// Makes `scanout` the kernel's display. Refused, and handed back, when there is one already.
pub fn install<S: Scanout + 'static>(scanout: S) -> Result<(), S> {
    let mut display = DISPLAY.lock();
    if display.is_some() {
        return Err(scanout);
    }
    *display = Some(Box::new(Display::new(Box::new(scanout))));
    Ok(())
}

/// The display's mode; `None` without a display.
pub fn mode() -> Option<Mode> {
    DISPLAY.lock().as_ref().map(|display| display.mode())
}

/// How many flushes have shown something on the display.
pub fn flushes() -> u64 {
    DISPLAY
        .lock()
        .as_ref()
        .map_or(0, |display| display.flushes())
}

/// Lends `draw` the display's framebuffer and returns what it returns; `None` without a
/// display.
pub fn draw<R>(draw: impl FnOnce(&mut Framebuffer<'_>) -> R) -> Option<R> {
    DISPLAY.lock().as_mut().map(|display| display.draw(draw))
}

/// Shows what was drawn on the display since the last flush ([`Display::flush`]); 0 without a
/// display.
pub fn flush<P: Platform>() -> Result<usize, &'static str> {
    let mut display = DISPLAY.lock();
    let mut deadline = Deadline::after::<P>(FLUSH_TIMEOUT_MS);
    display
        .as_mut()
        .map_or(Ok(0), |display| display.flush(&mut deadline))
}

/// Draws the first frame, the whole screen in [`FIRST_FRAME_COLOUR`], and shows it. Returns the
/// display's mode; `None` without a display.
pub fn show_first_frame<P: Platform>() -> Result<Option<Mode>, &'static str> {
    let Some(mode) = draw(|framebuffer| {
        let mode = framebuffer.mode();
        framebuffer.fill(mode.rect(), FIRST_FRAME_COLOUR);
        mode
    }) else {
        return Ok(None);
    };
    flush::<P>().map(|_| Some(mode))
}

/// The display's framebuffer, lent by [`draw`].
pub struct Framebuffer<'a> {
    pixels: &'a mut [u32],
    mode: Mode,
    damage: &'a mut Damage,
}

impl Framebuffer<'_> {
    /// Its mode: `mode().width` pixels per row.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The pixels, row after row, each `0x00RRGGBB`. What is written here shows only once
    /// marked with [`Self::damage`] and flushed.
    pub fn pixels(&mut self) -> &mut [u32] {
        self.pixels
    }

    /// Records that the pixels inside `rect` changed.
    pub fn damage(&mut self, rect: Rect) {
        self.damage
            .add(rect.clipped(self.mode.width, self.mode.height));
    }

    /// Fills the part of `rect` on the screen with `colour` and records it as damaged.
    pub fn fill(&mut self, rect: Rect, colour: u32) {
        let rect = rect.clipped(self.mode.width, self.mode.height);
        let width = self.mode.width as usize;
        for row in rect.y..rect.bottom() {
            let start = row as usize * width + rect.x as usize;
            if let Some(pixels) = self.pixels.get_mut(start..start + rect.width as usize) {
                pixels.fill(colour);
            }
        }
        self.damage.add(rect);
    }
}

impl core::fmt::Debug for Framebuffer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Framebuffer")
            .field("mode", &self.mode)
            .field("damage", &self.damage)
            .finish()
    }
}

/// FNV-1a, 64 bits, over the pixels' little-endian bytes: the hash the display tests compare a
/// frame by (docs/ux/desktop-v0.3.md, section 8).
pub fn pixel_hash(pixels: &[u32]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    pixels
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

#[cfg(test)]
mod tests;
